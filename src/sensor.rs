use std::io;
use std::path::Path;
use std::{fmt::Display, pin::Pin};

use bytes::{BufMut, BytesMut};
use io::ErrorKind;

use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::codec::{Decoder, Encoder};

use async_stream::try_stream;
use futures::stream::StreamExt;
use futures_core::stream::Stream;

pub const SENSOR_VID: u16 = 0x0403;
pub const SENSOR_PID: u16 = 0x6001;

struct SensorIO;

pub struct Sensor {
    port: SerialStream,
}

#[derive(Clone, Copy, Debug)]
pub struct RawSensorData {
    pub time1: u32,
    pub time2: u32,
    pub temp: i16,
}

#[derive(Clone, Copy, Debug)]
pub struct SensorData {
    pub distance: u32,
}

impl Display for SensorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} cm", self.distance)
    }
}

impl From<RawSensorData> for SensorData {
    fn from(raw_data: RawSensorData) -> Self {
        // 343.2 m/s = 0.03432 cm/µs, divide by 2 since we only need one way
        let distance = raw_data.time2 * 3432 / 100_000 / 2;

        Self { distance }
    }
}

impl Decoder for SensorIO {
    type Item = RawSensorData;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() > 20 {
            src.clear();
            return Ok(None);
        }

        let newline = src.as_ref().iter().position(|b| *b == b'\r');
        if let Some(n) = newline {
            let line = src.split_to(n);
            src.clear();

            log::trace!("[sensor data]: raw: {:?}", line);

            if n < 14 {
                log::trace!("[sensor data]: short read");
                return Ok(None);
            }

            let line = std::str::from_utf8(&line)
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?
                .trim();

            let time1: u32 = line[0..5]
                .parse()
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
            let time2: u32 = line[6..11]
                .parse()
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
            let temp: i16 = line[13..]
                .parse()
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;

            Ok(Some(RawSensorData { time1, time2, temp }))
        } else {
            Ok(None)
        }
    }
}

impl Encoder<Vec<u8>> for SensorIO {
    type Error = io::Error;

    fn encode(&mut self, item: Vec<u8>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.put(item.as_ref());
        dst.put_u8(b'\r');
        Ok(())
    }
}

impl Sensor {
    pub fn into_stream(self) -> Pin<Box<impl Stream<Item = io::Result<SensorData>>>> {
        Box::pin(try_stream! {

            let mut s = self.into_raw_stream();

            while let Some(raw_sensor_data) = s.next().await {
                let raw_sensor_data = raw_sensor_data?;
                yield raw_sensor_data.into();
            }
        })
    }

    pub fn into_raw_stream(self) -> Pin<Box<impl Stream<Item = io::Result<RawSensorData>>>> {
        let mut framed = SensorIO.framed(self.port);

        Box::pin(try_stream! {

            loop {

                let read_response = tokio::time::timeout(std::time::Duration::from_secs(1), framed.next()).await;
                match read_response {
                    Err(_) => {
                        println!("Read timed out");
                        log::error!("[sensor]: read timed out");
                        return;
                    },
                    Ok(response) => {
                        if let Some(raw_sensor_data) = response {
                            let raw_sensor_data = raw_sensor_data?;
                            yield raw_sensor_data;
                        } else {
                            log::trace!("[sensor]: stream ended");
                            return;
                        }
                    }
                }
            }
        })
    }
}

pub fn open<P>(path: P) -> io::Result<Sensor>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_str().unwrap().to_string();

    let port = tokio_serial::new(path, 38400).open_native_async()?;

    Ok(Sensor { port })
}
