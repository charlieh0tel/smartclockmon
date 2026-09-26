//! A serial port transport.

use std::io::Read;
use std::io::Write;
use std::time::Duration;

use crate::error::Result;
use crate::transport::Transport;
use crate::types::BaudRate;
use crate::types::Framing;

/// Serial line settings.  The receiver stores its own settings in
/// non-volatile memory, so these must match whatever it was last told,
/// not necessarily the factory default of 9600 8N1.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Device path, such as `/dev/ttyUSB0`.  Prefer a
    /// `/dev/serial/by-id/...` path, which survives re-enumeration.
    pub path: String,
    /// Line rate.  The receiver stores its own, so this is not
    /// necessarily the factory default.
    pub baud: BaudRate,
    /// Character framing: settable on the 58503A, fixed at 7O1 on the
    /// Z3801A.
    pub framing: Framing,
    /// How long a single read waits before returning empty.
    pub read_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path: "/dev/ttyUSB0".to_owned(),
            baud: BaudRate::B19200,
            framing: Framing::EightNone,
            read_timeout: Duration::from_millis(250),
        }
    }
}

/// A receiver reached over a serial port.
#[derive(Debug)]
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
    path: String,
}

impl SerialTransport {
    /// Open the port, at the settings' framing, with no flow control.
    pub fn open(settings: &Settings) -> Result<Self> {
        let data_bits = match settings.framing.data_bits() {
            7 => serialport::DataBits::Seven,
            _ => serialport::DataBits::Eight,
        };
        let parity = match settings.framing.odd_parity() {
            None => serialport::Parity::None,
            Some(true) => serialport::Parity::Odd,
            Some(false) => serialport::Parity::Even,
        };
        let port = serialport::new(&settings.path, settings.baud.get())
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(settings.read_timeout)
            .open()?;
        Ok(Self {
            port,
            path: settings.path.clone(),
        })
    }
}

impl Read for SerialTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.port.read(buf) {
            // A read timeout means no bytes yet, not a failure.  The
            // session layer decides when silence has gone on too long.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }
}

impl Write for SerialTransport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.port.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.port.flush()
    }
}

impl Transport for SerialTransport {
    fn describe(&self) -> String {
        self.path.clone()
    }
}
