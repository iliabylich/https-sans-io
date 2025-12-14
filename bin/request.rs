use anyhow::Result;

#[cfg(feature = "blocking")]
fn main() -> Result<()> {
    println!("Blocking version");

    use https_sansio::BlockingConnection;
    let response = BlockingConnection::get("myip.ibylich.dev", 443, "/")?;
    println!("Response: {response:?}");
    Ok(())
}

#[cfg(feature = "poll")]
fn main() -> Result<()> {
    println!("Poll version");

    use https_sansio::{EventsOrResponse, PollConnection};
    let mut conn = PollConnection::get("myip.ibylich.dev", 443, "/")?;

    use libc::{POLLERR, POLLIN, POLLOUT, poll, pollfd};
    use std::os::fd::AsRawFd;

    let mut fds = [pollfd {
        fd: conn.as_raw_fd(),
        events: POLLIN | POLLOUT,
        revents: 0,
    }];

    fn do_poll(fds: &mut [pollfd; 1]) -> (bool, bool) {
        let res = unsafe { poll(fds.as_mut_ptr(), 1, -1) };
        assert!(res == 1);
        let readable = fds[0].revents & POLLIN != 0;
        let writable = fds[0].revents & POLLOUT != 0;
        assert_eq!(fds[0].revents & POLLERR, 0);
        (readable, writable)
    }

    let response = loop {
        match conn.events()? {
            EventsOrResponse::Events(events) => {
                fds[0].events = events;
            }
            EventsOrResponse::Response(response) => break response,
        }
        let (readable, writable) = do_poll(&mut fds);

        if let Some(response) = conn.poll(readable, writable)? {
            break response;
        };
    };

    println!("Response: {response:?}");
    Ok(())
}

#[cfg(feature = "io-uring")]
fn main() -> Result<()> {
    println!("io_uring version");

    use https_sansio::{IoUringConnection, SqeOrResponse};
    use io_uring::IoUring;

    let mut ring = IoUring::new(10)?;
    let mut conn = IoUringConnection::get("myip.ibylich.dev", 443, "/")?;

    let response = loop {
        match conn.next_sqe()? {
            SqeOrResponse::Sqe(sqe) => {
                unsafe { ring.submission().push(&sqe)? };
            }
            SqeOrResponse::Response(response) => break response,
        }

        ring.submit_and_wait(1)?;

        while let Some(cqe) = ring.completion().next() {
            conn.process_cqe(cqe)?;
        }
    };

    println!("Response: {response:?}");
    Ok(())
}
