//! What the service answers on its socket: itself, and every sensor's
//! latest reading.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use smartclock::protocol::Message;
use smartclock::sensors::Info;
use smartclock::sensors::Latest;
use smartclock::sensors::Op;
use smartclock::sensors::VERSION;
use smartclock::server::Service;

/// The latest readings, written by the read loop and read by clients.
pub type Shared = Arc<Mutex<Latest>>;

/// The service, as its socket's clients see it.
#[derive(Debug)]
pub struct Answers {
    /// What it says about itself.
    pub info: Info,
    /// Every sensor's latest reading.
    pub latest: Shared,
}

impl Service for Answers {
    type Op = Op;

    fn answer(&self, id: String, op: Op) -> Message {
        let value = match op {
            Op::SensorInfo => serde_json::to_value(&self.info),
            Op::SensorLatest => {
                // A copy, so the lock is not held while it is written out.
                let latest = self
                    .latest
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                serde_json::to_value(latest)
            }
        };
        match value {
            Ok(value) => Message::ok_in(VERSION, id, value),
            Err(e) => Message::err_in(VERSION, id, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Answers;
    use smartclock::client::Daemon;
    use smartclock::link::listen_scratch;
    use smartclock::sensors::Info;
    use smartclock::sensors::Latest;
    use smartclock::sensors::Reading;
    use smartclock::server::serve;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[test]
    fn a_client_is_told_the_latest_readings() {
        let info = Info {
            version: "test".to_owned(),
            every_s: 10.0,
            log: "/nowhere".to_owned(),
        };
        let latest = Arc::new(Mutex::new(Latest {
            every_s: 10.0,
            readings: vec![Reading {
                name: "room".to_owned(),
                quantity: "temperature".to_owned(),
                unit: "C".to_owned(),
                source: "/sys/x/temp1_input".to_owned(),
                at: None,
                value: Some(21.5),
                error: None,
            }],
        }));
        let (listener, scratch) = listen_scratch("sensord").expect("listen");
        let answers = Arc::new(Answers {
            info: info.clone(),
            latest: Arc::clone(&latest),
        });
        std::thread::spawn(move || serve(vec![listener], "test", answers));
        let mut client = Daemon::connect(scratch.endpoint()).expect("connect");
        assert_eq!(client.sensor_info().expect("info"), info);
        let got = client.sensor_readings().expect("readings");
        assert_eq!(got.readings[0].value, Some(21.5));
        // The receiver daemon's requests are refused, not answered in
        // this service's terms.
        assert!(client.info().is_err());
        assert!(client.status().is_err());
    }
}
