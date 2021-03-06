use std::time::Duration;
use std::{fmt::Display, pin::Pin};
use std::{io, path::Path};

use bytes::{BufMut, BytesMut};

use async_stream::try_stream;
use futures::stream::StreamExt;
use futures::SinkExt;
use futures_core::stream::Stream;

use tokio_serial::Serial;
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
    port: Serial,
}

impl Decoder for BuzzerIO {
    type Item = BuzzerState;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let newline = src.as_ref().iter().position(|b| *b == b'\r');
        if let Some(n) = newline {
            let line = src.split_to(n);
            src.clear();
            let state: BuzzerState;
            if line == b"!00FE"[..] {
                state = BuzzerState::Pressed;
            } else {
                state = BuzzerState::Released;
            }
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }
}

impl Encoder for BuzzerIO {
    type Item = String;
    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
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

                tokio::time::delay_for(Duration::from_millis(50)).await;
            }
        })
    }
}

pub fn open<P>(path: P) -> io::Result<Buzzer>
where
    P: AsRef<Path>,
{
    let settings = tokio_serial::SerialPortSettings {
        baud_rate: 38400,
        ..Default::default()
    };

    let port = tokio_serial::Serial::from_path(path, &settings)?;

    Ok(Buzzer { port })
}
