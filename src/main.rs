use core::fmt;
use core::time::Duration;

use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::thread;

use time::ext::NumericalDuration;
use time::format_description;
use time::OffsetDateTime;
use time::Time;

use gpio_cdev::Line;
use gpio_cdev::LineHandle;
use gpio_cdev::LineRequestFlags;

use simplelog::ColorChoice;
use simplelog::CombinedLogger;
use simplelog::ConfigBuilder;
use simplelog::LevelFilter;
use simplelog::TermLogger;
use simplelog::TerminalMode;
use simplelog::WriteLogger;

use log::debug;
use log::error;
use log::info;
use log::warn;

use clap::Parser;
use clap::Subcommand;

mod config;

type Error = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, Error>;

const DEFAULT_CONFIG_FILE_NAME: &str = "config.toml";
const DEFAULT_LOG_FILE_NAME: &str = "water.log";
/// Run a pump test for one second by default.
const DEFAULT_TEST_SECS: f64 = 1.0;

const CONSUMER: &str = "water";

#[derive(Debug)]
struct Pin {
    /// The name of the pin.
    name: String,
    /// The GPIO line to use.
    line: Line,
    /// The handle for controlling the pin.
    handle: Option<LineHandle>,
}
impl Pin {
    fn new(name: &str, line: Line, enable: bool) -> Result<Pin> {
        let handle = {
            if enable {
                Some(line.request(LineRequestFlags::OUTPUT, 0, CONSUMER)?)
            } else {
                None
            }
        };
        Ok(Pin {
            name: name.to_string(),
            line,
            handle,
        })
    }
    fn set_value(&self, value: u8) -> Result<()> {
        debug!("setting pin {} to {}", self.name, value);
        self.set_value_raw(value)
    }
    fn set_value_raw(&self, value: u8) -> Result<()> {
        if let Some(handle) = &self.handle {
            handle.set_value(value)?;
        }
        Ok(())
    }
    fn create_pulse(&self, duration: Duration) -> Result<()> {
        self.set_value(1)?;
        thread::sleep(duration);
        self.set_value(0)?;
        Ok(())
    }
    fn is_enabled(&self) -> bool {
        self.handle.is_some()
    }
}
impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pin {} at device {} with offset {}",
            self.name,
            self.line.chip().path().to_string_lossy(),
            self.line.offset()
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
    fn new(name: &str, pin: Pin, ml_per_s: f64, ml_per_day: f64) -> Result<Pump> {
        Ok(Pump {
            name: name.to_string(),
            pin,
            ml_per_s,
            ml_per_day,
        })
    }
    fn pump(&self, duration: Duration) -> Result<()> {
        self.pin.create_pulse(duration)?;
        Ok(())
    }
    fn pump_for_secs(&self, secs: f64) -> Result<()> {
        let name = &self.name;
        if 0.0 <= secs && secs <= 30.0 {
            // TODO use checked conversion when stabilized
            let duration = Duration::from_secs_f64(secs);
            self.pump(duration)?;
        } else {
            warn!("{name}: pump duration {secs:.1}s out of range (min 0s, max 30s), doing nothing",);
        }
        Ok(())
    }
    fn water(&self) -> Result<()> {
        let name = &self.name;
        let ml = self.ml_per_day;
        let ml_per_s = self.ml_per_s;
        let secs = ml / ml_per_s;
        info!("{name}: pumping {ml:.0}mL in {secs:.1}s at {ml_per_s:.1}mL/s");
        self.pump_for_secs(secs)?;
        Ok(())
    }
}
impl fmt::Display for Pump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pump {} ({}), {:.1} mL/day at {:.1} mL/s on {}",
            self.name,
            if self.pin.is_enabled() {
                "enabled"
            } else {
                "disabled"
            },
            self.ml_per_day,
            self.ml_per_s,
            self.pin
        )
    }
}

fn run(pumps: &[Pump], watering_time: Time) -> Result<()> {
    // Check date and time once per second.
    let sleep_duration = Duration::from_millis(1_000);

    // Maximal error of a sleep duration before assuming a time jump.
    let delta_t = Duration::from_millis(10_000);

    // Make a short pause between running successive pumps.
    let pause_duration = Duration::from_millis(1_000);

    let format = format_description::parse(
        "[year]-[month]-[day] \
         [hour]:[minute]:[second] \
         [offset_hour sign:mandatory]:[offset_minute]",
    )?;

    'outer: loop {
        let mut t = OffsetDateTime::now_utc();
        let mut watering_date_time = t.replace_time(watering_time);
        if t >= watering_date_time {
            watering_date_time += 1.days();
        }
        info!("waiting for {}", watering_date_time.format(&format)?);
        while t < watering_date_time {
            thread::sleep(sleep_duration);
            let new_t = OffsetDateTime::now_utc();

            // Error of sleep duration.
            let e = new_t - (t + sleep_duration);

            if e.abs() > delta_t {
                info!("time jumped, restarting wait");
                continue 'outer;
            }
            t = new_t;
        }

        info!(
            "starting watering at {}",
            OffsetDateTime::now_utc().format(&format)?
        );
        for pump in pumps {
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

#[derive(Clone, Debug)]
struct PumpNotFoundError {
    pump_name: String,
}
impl std::error::Error for PumpNotFoundError {}
impl fmt::Display for PumpNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pump with name {} not found", self.pump_name)
    }
}

fn test(test_args: &TestArgs, pumps: &[Pump]) -> Result<()> {
    let pump_name = &test_args.pump;
    if let Some(pump) = pumps.iter().find(|pump| &pump.name == pump_name) {
        let secs = test_args.secs.unwrap_or(DEFAULT_TEST_SECS);
        info!("testing pump {pump_name} for {secs}s");
        pump.pump_for_secs(secs)
    } else {
        error!("there is no pump with name {pump_name}");
        let pump_name = pump_name.to_string();
        Err(Box::new(PumpNotFoundError { pump_name }))
    }
}

#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long)]
    /// Configuration file.
    ///
    /// Default is `config.toml` in the current directory.
    config_file: Option<String>,
    #[clap(long)]
    /// Log file.
    ///
    /// Default is `water.log` in the current directory.
    log_file: Option<String>,
    #[clap(short, long)]
    debug: bool,
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run,
    Test(TestArgs),
}
impl Command {
    fn mode_name(&self) -> &'static str {
        match self {
            Command::Run => "run",
            Command::Test(_) => "test",
        }
    }
}

#[derive(Parser, Debug)]
struct TestArgs {
    pump: String,
    secs: Option<f64>,
}

fn find_gpio_line(name: &str) -> Result<Line> {
    for chip in gpio_cdev::chips()? {
        let mut chip = chip?;
        for offset in 0..chip.num_lines() {
            if let Ok(line) = chip.get_line(offset) {
                let info = line.info()?;
                if let Some(line_name) = info.name() {
                    if line_name.starts_with(name) {
                        return Ok(line);
                    }
                }
            }
        }
    }
    Err(format!("gpio line {} not found", name).into())
}

fn find_pin(name: &str, enable: bool) -> Result<Pin> {
    let line = find_gpio_line(name)?;
    Ok(Pin::new(name, line, enable)?)
}

fn main() -> Result<()> {
    let args = Args::parse();

    let config_file_name = args
        .config_file
        .unwrap_or(DEFAULT_CONFIG_FILE_NAME.to_string());
    let mut file = File::open(config_file_name)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: config::Config = toml::from_str(&contents)?;

    let log_file_name = args.log_file.unwrap_or(DEFAULT_LOG_FILE_NAME.to_string());
    let log_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_file_name)?;
    let log_config = ConfigBuilder::new()
        .set_time_format_str("%F %T%.3f")
        .set_thread_level(LevelFilter::Off)
        .build();
    let level_filter = if args.debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let file_logger = WriteLogger::new(level_filter, log_config.clone(), log_file);
    if cfg!(feature = "term_logger") {
        let term_logger = TermLogger::new(
            LevelFilter::Debug,
            log_config,
            TerminalMode::Mixed,
            ColorChoice::Never,
        );
        CombinedLogger::init(vec![file_logger, term_logger])?;
    } else {
        CombinedLogger::init(vec![file_logger])?;
    }

    info!("starting in {} mode", args.command.mode_name());

    let config_time_format = format_description::parse("[hour]:[minute]:[second]")?;
    let time_string = config.timing.daily_start_time.to_string();
    let watering_time = Time::parse(&time_string, &config_time_format)?;

    let mut pumps = Vec::new();
    for (name, pump_config) in config.pumps {
        let pin = find_pin(&pump_config.pin_name, pump_config.enable)?;
        let pump = Pump::new(&name, pin, pump_config.ml_per_s, pump_config.ml_per_day)?;
        info!("configured {pump}");
        pumps.push(pump);
    }

    match args.command {
        Command::Run => run(&pumps, watering_time),
        Command::Test(test_args) => test(&test_args, &pumps),
    }
}
