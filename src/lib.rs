use anyhow::{Context as _, Result};
use rustls::{
    ClientConfig,
    client::UnbufferedClientConnection,
    pki_types::ServerName,
    unbuffered::{
        AppDataRecord, ConnectionState, EncodeError, EncryptError, InsufficientSizeError,
        UnbufferedStatus,
    },
};
use std::sync::Arc;

pub struct FSM {
    conn: UnbufferedClientConnection,
    response: Vec<u8>,

    incoming_tls: Vec<u8>,
    outgoing_tls: Vec<u8>,

    incoming_start: usize,
    incoming_end: usize,
    outgoing_start: usize,
    outgoing_end: usize,

    we_closed: bool,
    sent_request: bool,
    received_response: bool,
}

pub enum HttpsAction<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
    Done(Vec<u8>),
}

impl FSM {
    pub fn new(config: Arc<ClientConfig>, name: ServerName<'static>) -> Result<Self> {
        Ok(Self {
            conn: UnbufferedClientConnection::new(config.clone(), name)?,
            response: vec![],

            incoming_tls: vec![0; INCOMING_TLS_BUFSIZE],
            outgoing_tls: vec![0; OUTGOING_TLS_INITIAL_BUFSIZE],

            incoming_start: 0,
            incoming_end: 0,
            outgoing_start: 0,
            outgoing_end: 0,

            we_closed: false,
            sent_request: false,
            received_response: false,
        })
    }

    pub fn next_action(&mut self) -> Result<HttpsAction<'_>> {
        loop {
            println!(
                "Process TLS records: {:?}",
                &self.incoming_tls[self.incoming_start..self.incoming_end].len()
            );

            let UnbufferedStatus { discard, state } = self.conn.process_tls_records(
                &mut self.incoming_tls[self.incoming_start..self.incoming_end],
            );

            self.incoming_start += discard;

            let state = state.context("malformed internal state")?;

            match dbg!(state) {
                ConnectionState::ReadTraffic(mut state) => {
                    while let Some(res) = state.next_record() {
                        let AppDataRecord {
                            discard: new_discard,
                            payload,
                        } = res.context("failed to get AppDataRecord")?;

                        self.incoming_start += new_discard;

                        println!("Received {}", String::from_utf8_lossy(payload));

                        self.response.extend_from_slice(payload);

                        self.received_response = true;
                    }
                }

                ConnectionState::EncodeTlsData(mut state) => {
                    let written = match state.encode(&mut self.outgoing_tls[self.outgoing_end..]) {
                        Ok(written) => written,

                        Err(EncodeError::InsufficientSize(InsufficientSizeError {
                            required_size,
                        })) => {
                            let new_len = self.outgoing_end + required_size;
                            self.outgoing_tls.resize(new_len, 0);
                            state.encode(&mut self.outgoing_tls[self.outgoing_end..])?
                        }

                        Err(e) => {
                            return Err(e.into());
                        }
                    };

                    self.outgoing_end += written;
                }

                ConnectionState::TransmitTlsData(mut state) => {
                    if let Some(mut may_encrypt) = state.may_encrypt_app_data() {
                        if !self.sent_request {
                            let request = make_request();
                            let written = may_encrypt
                                .encrypt(&request, &mut self.outgoing_tls[self.outgoing_end..])
                                .context("encrypted request does not fit in `outgoing_tls`")?;
                            self.outgoing_end += written;
                            self.sent_request = true;
                            eprintln!("queued HTTP request");
                        }
                    }

                    if self.outgoing_start == self.outgoing_end {
                        state.done();
                    } else {
                        return Ok(self.want_write());
                    }
                }

                ConnectionState::BlockedHandshake { .. } => {
                    self.resize_incoming_if_needed();
                    return Ok(self.want_read());
                }

                ConnectionState::WriteTraffic(mut may_encrypt) => {
                    if !self.sent_request {
                        panic!("dead branch?");
                        // let request = make_request();
                        // let written = may_encrypt
                        //     .encrypt(&request, &mut self.outgoing_tls[self.outgoing_end..])
                        //     .context("encrypted request does not fit in `outgoing_tls`")?;
                        // self.outgoing_end += written;
                        // self.sent_request = true;
                        // eprintln!("queued HTTP request");

                        // todo!();
                        // // send_tls(&mut sock, outgoing_tls, &mut outgoing_end)?;
                        // self.resize_incoming_if_needed();
                        // todo!()
                        // // recv_tls(&mut sock, incoming_tls, &mut incoming_end)?;
                    } else if !self.received_response {
                        // this happens in the TLS 1.3 case. the app-data was sent in the preceding
                        // `TransmitTlsData` state. the server should have already written a
                        // response which we can read out from the socket
                        self.resize_incoming_if_needed();

                        return Ok(self.want_read());
                    } else if !self.we_closed {
                        let written = match may_encrypt
                            .queue_close_notify(&mut self.outgoing_tls[self.outgoing_end..])
                        {
                            Ok(written) => written,

                            Err(EncryptError::InsufficientSize(InsufficientSizeError {
                                required_size,
                            })) => {
                                let new_len = self.outgoing_end + required_size;
                                self.outgoing_tls.resize(new_len, 0);
                                may_encrypt.queue_close_notify(
                                    &mut self.outgoing_tls[self.outgoing_end..],
                                )?
                            }

                            Err(e) => {
                                return Err(e.into());
                            }
                        };

                        self.outgoing_end += written;

                        self.we_closed = true;
                        return Ok(self.want_write());
                    } else {
                        self.resize_incoming_if_needed();

                        return Ok(self.want_read());
                    }
                }

                ConnectionState::PeerClosed => {}

                ConnectionState::Closed => {
                    assert!(self.sent_request);
                    assert!(self.received_response);
                    assert_eq!(self.incoming_start, self.incoming_end);
                    assert_eq!(0, self.outgoing_end);

                    return Ok(HttpsAction::Done(std::mem::take(&mut self.response)));
                }

                _ => unreachable!(),
            }
        }
    }

    fn resize_incoming_if_needed(&mut self) {
        if self.incoming_end == self.incoming_tls.len() {
            let new_len = self.incoming_tls.len() + INCOMING_TLS_BUFSIZE;
            self.incoming_tls.resize(new_len, 0);
            eprintln!("grew buffer to {} bytes", new_len);
        }
    }

    fn want_write(&self) -> HttpsAction<'_> {
        HttpsAction::Write(&self.outgoing_tls[self.outgoing_start..self.outgoing_end])
    }

    fn want_read(&mut self) -> HttpsAction<'_> {
        HttpsAction::Read(&mut self.incoming_tls[self.incoming_end..])
    }

    pub fn done_reading(&mut self, read: usize) {
        self.incoming_end += read;
    }

    pub fn done_writing(&mut self, written: usize) {
        self.outgoing_start += written;
        if self.outgoing_start == self.outgoing_end {
            self.outgoing_start = 0;
            self.outgoing_end = 0;
        }
    }
}

const SERVER_NAME: &str = "myip.ibylich.dev";

const KB: usize = 1024;
const INCOMING_TLS_BUFSIZE: usize = 16 * KB;
const OUTGOING_TLS_INITIAL_BUFSIZE: usize = KB;

fn make_request() -> Vec<u8> {
    format!("GET / HTTP/1.1\r\nHost: {SERVER_NAME}\r\nConnection: close\r\nAccept-Encoding: identity\r\n\r\n").into_bytes()
}
