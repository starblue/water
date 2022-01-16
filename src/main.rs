use core::fmt;
use core::time::Duration;

use std::error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::thread;

use time::ext::NumericalDuration;
use time::format_description;
use time::OffsetDateTime;
use time::Time;

#[cfg(feature = "gpio")]
use gpio_cdev::Chip;
#[cfg(feature = "gpio")]
use gpio_cdev::LineHandle;
#[cfg(feature = "gpio")]
use gpio_cdev::LineRequestFlags;

use simplelog::ColorChoice;
use simplelog::CombinedLogger;
use simplelog::ConfigBuilder;
use simplelog::LevelFilter;
use simplelog::TermLogger;
use simplelog::TerminalMode;
use simplelog::WriteLogger;

use log::info;
use log::warn;

mod config;

#[cfg(feature = "gpio")]
const CONSUMER: &str = "water";

#[derive(Debug)]
struct Pin {
    /// The name of the pin.
    name: String,
    /// Name of the device, typically `/dec/gpiochipN`.
    device: String,
    /// Pin offset within the device.
    offset: u32,
    /// The handle for controlling the pin.
    #[cfg(feature = "gpio")]
    handle: LineHandle,
}
impl Pin {
    #[cfg(feature = "gpio")]
    fn new(name: &str, device: &str, offset: u32) -> Result<Pin, gpio_cdev::Error> {
        let mut chip = Chip::new(device)?;
        let line = chip.get_line(offset)?;
        let handle = line.request(LineRequestFlags::OUTPUT, 0, CONSUMER)?;
        Ok(Pin {
            name: name.to_string(),
            device: device.to_string(),
            offset,
            handle,
        })
    }
    #[cfg(not(feature = "gpio"))]
    fn new(name: &str, device: &str, offset: u32) -> Result<Pin, gpio_cdev::Error> {
        Ok(Pin {
            name: name.to_string(),
            device: device.to_string(),
            offset,
        })
    }
}
impl Pin {
    fn set_value(&self, value: u8) -> Result<(), gpio_cdev::Error> {
        info!("setting pin {} to {}", self.name, value);
        self.set_value_raw(value)
    }
    #[cfg(feature = "gpio")]
    fn set_value_raw(&self, value: u8) -> Result<(), gpio_cdev::Error> {
        self.handle.set_value(value)
    }
    #[cfg(not(feature = "gpio"))]
    fn set_value_raw(&self, _value: u8) -> Result<(), gpio_cdev::Error> {
        Ok(())
    }
    fn create_pulse(&self, duration: Duration) -> Result<(), gpio_cdev::Error> {
        self.set_value(1)?;
        thread::sleep(duration);
        self.set_value(0)?;
        Ok(())
    }
}
impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "pin {} at device {} with offset {}",
            self.name, self.device, self.offset
        )
    }
}

struct Pump {
    /// The name of the pump.
    name: String,
    /// The pin for controlling the pump.
    pin: Pin,
    /// Amount of water pumped per second.
    ml_per_s: f64,
    /// Amount of water required per day.
    ml_per_day: f64,
}
impl Pump {
    fn new(
        name: &str,
        connector: &str,
        device: &str,
        offset: u32,
        ml_per_s: f64,
        ml_per_day: f64,
    ) -> Result<Pump, Box<dyn error::Error>> {
        Ok(Pump {
            name: name.to_string(),
            pin: Pin::new(connector, device, offset)?,
            ml_per_s,
            ml_per_day,
        })
    }
    fn water(&self) -> Result<(), Box<dyn error::Error>> {
        let name = &self.name;
        let seconds = self.ml_per_day / self.ml_per_s;
        if 1.0 <= seconds && seconds <= 30.0 {
            // TODO use checked conversion when stabilized
            let duration = Duration::from_secs_f64(seconds);
            info!("{name}: running for {seconds:.1}s");
            self.pin.create_pulse(duration)?;
        } else {
            warn!(
                "{name}: watering duration {seconds:.1}s out of range (min 1s, max 30s), doing nothing",
            );
        }
        Ok(())
    }
}
impl fmt::Display for Pump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "pump {}, {:.1} mL/day at {:.1} mL/s on {}",
            self.name, self.ml_per_day, self.ml_per_s, self.pin
        )
    }
}

fn main() -> Result<(), Box<dyn error::Error>> {
    let mut file = File::open("config.toml")?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: config::Config = toml::from_str(&contents)?;

    let log_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open("water.log")?;
    let log_config = ConfigBuilder::new()
        .set_time_format_str("%F %T%.3f")
        .build();
    let file_logger = WriteLogger::new(LevelFilter::Info, log_config.clone(), log_file);
    if cfg!(feature = "term_logger") {
        let term_logger = TermLogger::new(
            LevelFilter::Info,
            log_config,
            TerminalMode::Mixed,
            ColorChoice::Never,
        );
        CombinedLogger::init(vec![file_logger, term_logger])?;
    } else {
        CombinedLogger::init(vec![file_logger])?;
    }

    info!("starting");

    let mut pumps = Vec::new();
    for (name, pump_config) in config.pumps {
        let config::Pump {
            connector,
            device,
            offset,
            ml_per_s,
            ml_per_day,
        } = pump_config;
        let pump = Pump::new(&name, &connector, &device, offset, ml_per_s, ml_per_day)?;
        info!("adding {pump}");
        pumps.push(pump);
    }

    // Check date and time once per second.
    let sleep_duration = Duration::from_millis(1_000);

    // Make a short pause between running successive pumps.
    let pause_duration = Duration::from_millis(1_000);

    let format = format_description::parse(
        "[year]-[month]-[day] \
         [hour]:[minute]:[second] \
         [offset_hour sign:mandatory]:[offset_minute]",
    )?;

    let config_time_format = format_description::parse("[hour]:[minute]:[second]")?;
    let time_string = config.timing.daily_start_time.to_string();
    let watering_time = Time::parse(&time_string, &config_time_format)?;

    let now = OffsetDateTime::now_local()?;
    let mut next_date_time = now.replace_time(watering_time);
    if next_date_time <= now {
        next_date_time += 1.days();
    }
    loop {
        info!("waiting for {}", next_date_time.format(&format)?);
        while OffsetDateTime::now_utc() < next_date_time {
            thread::sleep(sleep_duration);
        }
        next_date_time += 1.days();

        info!(
            "starting watering at {}",
            OffsetDateTime::now_utc().format(&format)?
        );
        for pump in &pumps {
            if let Err(err) = pump.water() {
                warn!("pumping failed with error {err:?}");
            }
            thread::sleep(pause_duration);
        }
        info!(
            "finished watering at {}",
            OffsetDateTime::now_utc().format(&format)?
        );
    }
}
