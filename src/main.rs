use std::{
    io,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use actix::{Actor, Addr, Context, Handler, Message, StreamHandler};
use actix_broker::{BrokerIssue, BrokerSubscribe, SystemBroker};
use actix_web::{
    dev::{ServiceRequest, ServiceResponse},
    middleware, web, App, Error, HttpRequest, HttpResponse, HttpServer,
};
use actix_web_actors::ws;

use serde_json::json;

use futures_util::stream::StreamExt;

use serialport::SerialPortType::UsbPort;

use fnwalk::{
    buzzer::{self, BuzzerState, BUZZER_PID, BUZZER_VID},
    sensor::{self, SensorData, SENSOR_PID, SENSOR_VID},
};

type BrokerType = SystemBroker;

async fn buzzer_stream(device_actor_addr: Addr<DeviceActor>, buzzer_port: &str) -> io::Result<()> {
    let buzzer = buzzer::open(buzzer_port)?;
    let mut s = buzzer.into_stream();

    while let Some(state) = s.next().await {
        match state {
            Ok(state) => device_actor_addr.do_send(BuzzerMessage { state }),
            Err(err) => {
                return Err(err);
            }
        }
    }

    Ok(())
}

async fn sensor_stream(device_actor_addr: Addr<DeviceActor>, sensor_port: &str) -> io::Result<()> {
    let sensor = sensor::open(sensor_port)?;
    let mut s = sensor.into_stream();

    while let Some(state) = s.next().await {
        match state {
            Ok(sensor_data) => device_actor_addr.do_send(SensorMessage { sensor_data }),
            Err(err) => {
                return Err(err);
            }
        }
    }

    Ok(())
}

// demo buzzer stream: pressed once every 5 seconds
async fn demo_buzzer_stream(device_actor_addr: Addr<DeviceActor>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        device_actor_addr.do_send(BuzzerMessage {
            state: BuzzerState::Pressed,
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        device_actor_addr.do_send(BuzzerMessage {
            state: BuzzerState::Released,
        });
    }
}

// demo sensor stream: simulate a sinus wave
async fn demo_sensor_stream(device_actor_addr: Addr<DeviceActor>) {
    let start_ts = Instant::now();

    let duration = 10.0;

    loop {
        tokio::time::sleep(Duration::from_millis(17)).await;

        let now = Instant::now().duration_since(start_ts);

        let t = now.as_secs_f64();

        let distance = 300.0 + 250.0 * f64::sin(t * 4.0 / duration);
        let distance = distance as u32;

        device_actor_addr.do_send(SensorMessage {
            sensor_data: SensorData { distance },
        });
    }
}

#[derive(Clone, Debug, Message)]
#[rtype(result = "()")]
struct BuzzerMessage {
    state: BuzzerState,
}

#[derive(Clone, Debug, Message)]
#[rtype(result = "()")]
struct SensorMessage {
    sensor_data: SensorData,
}

struct DeviceActor;

impl Actor for DeviceActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.subscribe_async::<BrokerType, BuzzerMessage>(ctx);
        self.subscribe_async::<BrokerType, SensorMessage>(ctx);
    }
}

impl Handler<BuzzerMessage> for DeviceActor {
    type Result = ();

    fn handle(&mut self, item: BuzzerMessage, _ctx: &mut Self::Context) {
        log::debug!("[buzzer]: {:?}", item.state);
        self.issue_async::<BrokerType, _>(item);
    }
}

impl Handler<SensorMessage> for DeviceActor {
    type Result = ();

    fn handle(&mut self, item: SensorMessage, _ctx: &mut Self::Context) {
        log::debug!("[sensor]: {:?}", item.sensor_data);
        self.issue_async::<BrokerType, _>(item);
    }
}

struct WsSession;

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.subscribe_async::<BrokerType, BuzzerMessage>(ctx);
        self.subscribe_async::<BrokerType, SensorMessage>(ctx);
    }
}

impl Handler<BuzzerMessage> for WsSession {
    type Result = ();

    fn handle(&mut self, item: BuzzerMessage, ctx: &mut Self::Context) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let msg = json!({
            "type": "buzzer",
            "ts": ts,
            "pressed": matches!(item.state, BuzzerState::Pressed)
        });

        ctx.text(msg.to_string());
    }
}

impl Handler<SensorMessage> for WsSession {
    type Result = ();

    fn handle(&mut self, item: SensorMessage, ctx: &mut Self::Context) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let msg = json!({
            "type": "sensor",
            "ts": ts,
            "distance": item.sensor_data.distance
        });

        ctx.text(msg.to_string());
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, _msg: Result<ws::Message, ws::ProtocolError>, _ctx: &mut Self::Context) {
        // ignore
    }
}

/// Entry point for our websocket route
async fn ws_handler(req: HttpRequest, stream: web::Payload) -> Result<HttpResponse, Error> {
    ws::start(WsSession, &req, stream)
}

async fn wait_for_serial_device(vid: u16, pid: u16) -> String {
    loop {
        let ports = serialport::available_ports().unwrap();

        let buzzer_port = ports.iter().find_map(|p| match &p.port_type {
            UsbPort(u) if u.vid == vid && u.pid == pid => Some(p.port_name.clone()),
            _ => None,
        });

        match buzzer_port {
            Some(buzzer_port) => {
                return buzzer_port;
            }
            None => {
                log::trace!("No serial device found with {:04x}:{:04x}", vid, pid);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn buzzer_loop(device_actor_addr: Addr<DeviceActor>) -> io::Result<()> {
    loop {
        log::info!("[buzzer]: Searching for device");
        let buzzer_port = wait_for_serial_device(BUZZER_VID, BUZZER_PID).await;
        log::info!("[buzzer]: detected at port {}", buzzer_port);

        let stream_result = buzzer_stream(device_actor_addr.clone(), &buzzer_port).await;
        match stream_result {
            Err(err) => log::error!("[buzzer]: stream error: {}", err.to_string()),
            Ok(()) => log::info!("[buzzer]: stream ended"),
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn sensor_loop(device_actor_addr: Addr<DeviceActor>) -> io::Result<()> {
    loop {
        log::info!("[sensor]: Searching for device");
        let sensor_port = wait_for_serial_device(SENSOR_VID, SENSOR_PID).await;
        log::info!("[sensor]: detected at port {}", sensor_port);

        let stream_result = sensor_stream(device_actor_addr.clone(), &sensor_port).await;
        match stream_result {
            Err(err) => log::error!("[sensor]: stream error: {}", err.to_string()),
            Ok(()) => log::info!("[sensor]: stream ended"),
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "actix_web=info,fnwalk=info");
    }
    env_logger::init();

    let device_actor_addr = DeviceActor.start();

    let demo_mode = std::env::args().any(|arg| arg == "--demo");
    let demo_buzzer = demo_mode || std::env::args().any(|arg| arg == "--demo-buzzer");
    let demo_sensor = demo_mode || std::env::args().any(|arg| arg == "--demo-sensor");

    if demo_buzzer {
        tokio::spawn(demo_buzzer_stream(device_actor_addr.clone()));
    } else {
        tokio::spawn(buzzer_loop(device_actor_addr.clone()));
    }

    if demo_sensor {
        tokio::spawn(demo_sensor_stream(device_actor_addr.clone()));
    } else {
        tokio::spawn(sensor_loop(device_actor_addr.clone()));
    }

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .service(
                web::scope("/fnwalk/api")
                    .service(web::resource("/ws").route(web::get().to(ws_handler))),
            )
            .service(
                actix_files::Files::new("/fnwalk", "./web")
                    .index_file("index.html")
                    .default_handler(|req: ServiceRequest| {
                        let (http_req, _payload) = req.into_parts();

                        async {
                            let response = actix_files::NamedFile::open("./web/index.html")?
                                .into_response(&http_req);
                            Ok(ServiceResponse::new(http_req, response))
                        }
                    }),
            )
            .service(web::resource("/").route(web::get().to(|| {
                HttpResponse::Found()
                    .append_header(("Location", "/fnwalk/"))
                    .finish()
            })))
    })
    .bind("127.0.0.1:8050")?
    .run()
    .await
}
