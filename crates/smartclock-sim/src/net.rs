//! The simulated receiver on a TCP connection.
//!
//! What the `smartclock-sim` binary serves, and what a test serves when
//! the thing under test opens its receiver by address rather than being
//! handed a transport.

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;

use crate::transport::SimTransport;

/// Shuttle bytes between the socket and the simulated receiver.
pub fn serve(mut stream: TcpStream, mut sim: SimTransport) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(20)))?;
    let mut from_client = [0u8; 512];
    let mut from_receiver = [0u8; 512];
    loop {
        match stream.read(&mut from_client) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                sim.write_all(&from_client[..n])?;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(e),
        }
        let n = sim.read(&mut from_receiver)?;
        if n > 0 {
            stream.write_all(&from_receiver[..n])?;
            stream.flush()?;
        }
    }
}
