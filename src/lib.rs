mod client_config;
mod fsm;
mod request;
mod response;

pub use crate::{
    fsm::{FSM, Wants},
    request::Request,
    response::Response,
};

#[cfg(feature = "blocking")]
mod blocking_connection;
#[cfg(feature = "blocking")]
pub use blocking_connection::BlockingConnection;

#[cfg(feature = "poll")]
mod poll_connection;
#[cfg(feature = "poll")]
pub use poll_connection::{EventsOrResponse, PollConnection};

#[cfg(feature = "io-uring")]
mod io_uring_connection;
#[cfg(feature = "io-uring")]
pub use io_uring_connection::{Cqe, IoUringConnection, Sqe, SqeOrResponse};
