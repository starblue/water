use std::collections::BTreeMap;
use std::str::FromStr;

use toml::value::Datetime;

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct Config {
    pub timing: Timing,
    pub pumps: BTreeMap<String, Pump>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Timing {
    /// Time when the daily watering is started.
    pub daily_start_time: Datetime,
}
impl Default for Timing {
    fn default() -> Self {
        Timing {
            daily_start_time: Datetime::from_str("07:30:00").unwrap(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct Pump {
    /// Name of the connector
    pub connector: String,
    /// Name of the device, typically `/dec/gpiochipN`.
    pub device: String,
    /// Pin offset within the device.
    pub offset: u32,
    /// Amount of water pumped per second.
    pub ml_per_s: f64,
    /// Amount of water required per day.
    pub ml_per_day: f64,
}
