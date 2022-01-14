use core::time::Duration;

use std::fs::File;
use std::thread;

use time::ext::NumericalDuration;
use time::format_description;
use time::OffsetDateTime;
use time::Time;

use gpio_cdev::Chip;
#[cfg(feature = "gpio")]
use gpio_cdev::LineHandle;
#[cfg(feature = "gpio")]
use gpio_cdev::LineRequestFlags;

use simplelog::ColorChoice;
use simplelog::CombinedLogger;
use simplelog::Config;
use simplelog::LevelFilter;
use simplelog::TermLogger;
use simplelog::TerminalMode;
use simplelog::WriteLogger;

use log::info;

#[cfg(feature = "gpio")]
const CONSUMER: &str = "water";

#[derive(Debug)]
struct Pin {
    name: String,
    #[cfg(feature = "gpio")]
    handle: LineHandle,
}
impl Pin {
    #[cfg(feature = "gpio")]
    fn new(chip: &mut Chip, offset: u32, name: &str) -> Result<Pin, gpio_cdev::Error> {
        let line = chip.get_line(offset)?;
        let handle = line.request(LineRequestFlags::OUTPUT, 0, CONSUMER)?;
        let name = name.to_string();
        Ok(Pin { name, handle })
    }
    #[cfg(not(feature = "gpio"))]
    fn new(_chip: &mut Chip, _offset: u32, name: &str) -> Result<Pin, gpio_cdev::Error> {
        let name = name.to_string();
        Ok(Pin { name })
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

#[allow(unused)]
const PUMP0: u32 = 13; // P8 11
#[allow(unused)]
const PUMP1: u32 = 12; // P8 12
#[allow(unused)]
const PUMP2: u32 = 15; // P8 15
#[allow(unused)]
const PUMP3: u32 = 14; // P8 16

fn main() -> Result<(), gpio_cdev::Error> {
    let term_logger = TermLogger::new(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Never,
    );
    let file_logger = WriteLogger::new(
        LevelFilter::Info,
        Config::default(),
        File::create("water.log").unwrap(),
    );
    CombinedLogger::init(vec![term_logger, file_logger]).unwrap();

    let mut chip = Chip::new("/dev/gpiochip1")?;

    let pin0 = Pin::new(&mut chip, PUMP0, "pump0")?;
    let pin1 = Pin::new(&mut chip, PUMP1, "pump1")?;
    let pin2 = Pin::new(&mut chip, PUMP2, "pump2")?;
    let pin3 = Pin::new(&mut chip, PUMP3, "pump3")?;

    // Check date and time once per second.
    let sleep_duration = Duration::from_millis(1_000);

    // Make a short pause between running successive pumps.
    let pause_duration = Duration::from_millis(1_000);

    // Run each pump for 6s to pump about 20mL.
    let pulse_duration = Duration::from_millis(6_000);

    let format = format_description::parse(
        "[year]-[month]-[day] \
         [hour]:[minute]:[second] \
         [offset_hour sign:mandatory]:[offset_minute]",
    )
    .unwrap();

    let watering_time = Time::from_hms(7, 0, 0).unwrap();

    let now = OffsetDateTime::now_local().unwrap();
    let mut next_date_time = now.replace_time(watering_time);
    if now > next_date_time {
        next_date_time += 1.days();
    }
    loop {
        info!("waiting for {}", next_date_time.format(&format).unwrap());
        while OffsetDateTime::now_utc() < next_date_time {
            thread::sleep(sleep_duration);
        }
        next_date_time += 1.days();

        pin0.create_pulse(pulse_duration)?;
        thread::sleep(pause_duration);
        pin1.create_pulse(pulse_duration)?;
        thread::sleep(pause_duration);
        pin2.create_pulse(pulse_duration)?;
        thread::sleep(pause_duration);
        pin3.create_pulse(pulse_duration)?;
    }
}
