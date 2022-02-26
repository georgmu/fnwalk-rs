use std::time::Duration;
use std::{fmt::Display, pin::Pin};
use std::{io, path::Path};

use bytes::{BufMut, BytesMut};

use async_stream::try_stream;
use futures::stream::StreamExt;
use futures::SinkExt;
use futures_core::stream::Stream;

use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::codec::{Decoder, Encoder};

pub const BUZZER_VID: u16 = 0x09ca;
pub const BUZZER_PID: u16 = 0x5544;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuzzerState {
    Released,
    Pressed,
}

impl Display for BuzzerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

struct BuzzerIO;

pub struct Buzzer {
    port: SerialStream,
}

impl Decoder for BuzzerIO {
    type Item = BuzzerState;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let newline = src.as_ref().iter().position(|b| *b == b'\r');

        newline.map_or(Ok(None), |n| {
            let line = src.split_to(n);
            src.clear();
            let state: BuzzerState = if line == b"!00FE"[..] {
                BuzzerState::Pressed
            } else {
                BuzzerState::Released
            };
            Ok(Some(state))
        })
    }
}

impl Encoder<String> for BuzzerIO {
    type Error = io::Error;

    fn encode(&mut self, item: String, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.put(item.as_bytes());
        dst.put_u8(b'\r');
        Ok(())
    }
}

impl Buzzer {
    pub fn into_stream(self) -> Pin<Box<impl Stream<Item = io::Result<BuzzerState>>>> {
        let mut framed = BuzzerIO.framed(self.port);

        Box::pin(try_stream! {

            let mut old_state = BuzzerState::Released;
            loop {
                framed.send("@00P0?".into()).await?;
                let read_response = tokio::time::timeout(std::time::Duration::from_secs(2), framed.next()).await;
                match read_response {
                    Err(_) => {
                        println!("Read timed out");
                        log::error!("[buzzer]: read timed out");
                    },
                    Ok(response) => {
                        if let Some(new_state) = response {
                            let new_state = new_state?;
                            if old_state != new_state {
                                old_state = new_state;
                                yield new_state;
                            }
                        }
                    }
                }

                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    }
}

pub fn open<P>(path: P) -> io::Result<Buzzer>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_str().unwrap().to_string();

    let port = tokio_serial::new(path, 38400).open_native_async()?;

    Ok(Buzzer { port })
}
