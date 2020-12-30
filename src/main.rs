use std::{io, io::ErrorKind, process::exit};

use actix::{Actor, Addr, Context, Handler, Message, StreamHandler};
use actix_broker::{BrokerIssue, BrokerSubscribe, SystemBroker};
use actix_web::{middleware, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;

use fnwalk::{
    buzzer::{self, Buzzer, BuzzerState, BUZZER_PID, BUZZER_VID},
    sensor::{self, Sensor, SensorData, SENSOR_PID, SENSOR_VID},
};
use futures_util::stream::StreamExt;

use serialport::SerialPortType::UsbPort;

type BrokerType = SystemBroker;

async fn buzzer_stream(device_actor_addr: Addr<DeviceActor>, buzzer: Buzzer) {
    let mut s = buzzer.into_stream();

    while let Some(state) = s.next().await {
        match state {
            Ok(state) => device_actor_addr.do_send(BuzzerMessage { state }),
            Err(err) => {
                println!("[buzzer] Error: {:?}", err);
                exit(1);
            }
        }
    }
}

async fn sensor_stream(device_actor_addr: Addr<DeviceActor>, sensor: Sensor) {
    let mut s = sensor.into_stream();

    while let Some(state) = s.next().await {
        match state {
            Ok(sensor_data) => device_actor_addr.do_send(SensorMessage { sensor_data }),
            Err(err) => {
                println!("[buzzer] Error: {:?}", err);
                exit(1);
            }
        }
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
        ctx.text(item.state.to_string());
    }
}

impl Handler<SensorMessage> for WsSession {
    type Result = ();

    fn handle(&mut self, item: SensorMessage, ctx: &mut Self::Context) {
        ctx.text(item.sensor_data.to_string());
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "actix_web=info");
    }
    env_logger::init();

    let ports = serialport::available_ports().unwrap();

    let buzzer_port = ports.iter().find_map(|p| match &p.port_type {
        UsbPort(u) if u.vid == BUZZER_VID && u.pid == BUZZER_PID => Some(p.port_name.clone()),
        _ => None,
    });

    let sensor_port = ports.iter().find_map(|p| match &p.port_type {
        UsbPort(u) if u.vid == SENSOR_VID && u.pid == SENSOR_PID => Some(p.port_name.clone()),
        _ => None,
    });

    let device_actor_addr = DeviceActor.start();

    if let Some(buzzer_port) = buzzer_port.as_ref() {
        log::info!("[buzzer]: Port {}", buzzer_port);
        let b = buzzer::open(buzzer_port)?;

        tokio::spawn(buzzer_stream(device_actor_addr.clone(), b));
    }

    if let Some(sensor_port) = sensor_port.as_ref() {
        log::info!("[sensor]: Port {}", sensor_port);
        let s = sensor::open(sensor_port)?;

        tokio::spawn(sensor_stream(device_actor_addr, s));
    }

    if buzzer_port.is_none() && sensor_port.is_none() {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            "Neither sensor nor buzzer found",
        ));
    }

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .service(web::resource("/api/ws").route(web::get().to(ws_handler)))
    })
    .bind("127.0.0.1:8050")?
    .run()
    .await
}
