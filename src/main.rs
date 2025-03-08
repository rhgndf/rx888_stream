mod fx3;
mod rx888;

use std::{
    collections::VecDeque,
    fmt::{Display, Formatter},
    fs::File,
    io::Write,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use bytemuck::cast_slice_mut;
use clap::{value_parser, Parser, Subcommand, ValueEnum};
use rusb::{Context, UsbContext};
use rusb_async::TransferPool;
use rx888::{
    rx888_send_argument, rx888_send_command, rx888_send_command_u64, ArgumentList, FX3Command,
    GPIOPin,
};

const FX3_VID: u16 = 0x04b4;
const FX3_BOOTLOADER_PID: u16 = 0x00f3;
const FX3_FIRMWARE_PID: u16 = 0x00f1;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum GainMode {
    High,
    Low,
}

/// RX888 USB streamer program
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Subcommand
    #[command(subcommand)]
    command: Option<Commands>,

    /// Firmware file to load
    #[arg(short, long, global = true)]
    firmware: Option<PathBuf>,

    /// Enable dithering
    #[arg(short, long, global = true, default_value_t = false)]
    dither: bool,

    /// Enable randomization
    #[arg(short, long, global = true, default_value_t = false)]
    randomize: bool,

    /// ADC sample rate
    #[arg(short, long, global = true, default_value_t = 50000000, value_parser = value_parser!(u32).range(10000000..150000000))]
    sample_rate: u32,

    /// VGA gain setting 0-127
    #[arg(short, long, global = true, default_value_t = 1, value_parser = value_parser!(u8).range(0..=127))]
    gain: u8,

    /// VGA gain mode high or low
    #[arg(short = 'm', long, global = true, default_value = "high")]
    gain_mode: GainMode,

    /// Attenuator setting 0-63
    #[arg(short, long, default_value_t = 0, value_parser = value_parser!(u8).range(0..=63))]
    attenuation: u8,

    /// HF Bias-T
    #[arg(long, global = true, default_value_t = false)]
    bias_hf: bool,

    /// VHF Bias-T
    #[arg(long, global = true, default_value_t = false)]
    bias_vhf: bool,

    /// PGA enable
    #[arg(long, global = true, default_value_t = false)]
    pga: bool,

    /// Output file, "-" is stdout
    #[arg(short, long, global = true)]
    output: Option<PathBuf>,

    /// Measurement mode, measures the ADC sample rate
    #[arg(long, global = true, default_value_t = false)]
    measure: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Accept from VHF input instead of HF input
    VHF {
        /// Tuner Frequency
        #[arg(long, display_order = 100, default_value_t = 145000000)]
        frequency: u64,

        /// Tuner LNA gain 0-29
        #[arg(long, display_order = 100, default_value_t = 29, value_parser = value_parser!(u8).range(0..=29))]
        vhf_lna: u8,

        /// Tuner VGA gain 0-15
        #[arg(long, display_order = 100, default_value_t = 15, value_parser = value_parser!(u8).range(0..=15))]
        vhf_vga: u8,

        /// Tuner sideband
        #[arg(long, display_order = 100, default_value_t = 0, value_parser = value_parser!(u8).range(0..=1))]
        vhf_sideband: u8,

        /// Tuner harmonic
        #[arg(long, display_order = 100, default_value_t = 0, value_parser = value_parser!(u8).range(0..=1))]
        vhf_harmonic: u8,
    },
}

struct Measurement {
    last_packet_time: Instant,
    packet_durations: VecDeque<Duration>,
    packet_sizes: VecDeque<usize>,
    total_duration: Duration,
    total_size: usize,
    last_display_time: Instant,
}

impl Measurement {
    fn new() -> Self {
        Self {
            last_packet_time: Instant::now(),
            packet_durations: VecDeque::with_capacity(100),
            packet_sizes: VecDeque::with_capacity(100),
            total_duration: Duration::from_secs(0),
            total_size: 0,
            last_display_time: Instant::now(),
        }
    }

    fn add_packet(&mut self, length: usize) {
        let now = Instant::now();
        let packet_time = now.duration_since(self.last_packet_time);
        self.last_packet_time = now;
        self.packet_durations.push_back(packet_time);
        self.packet_sizes.push_back(length);
        self.total_duration += packet_time;
        self.total_size += length;

        if self.packet_durations.len() > 1024 {
            self.total_duration -= self.packet_durations.pop_front().unwrap();
            self.total_size -= self.packet_sizes.pop_front().unwrap();
            self.packet_durations.pop_front();
            self.packet_sizes.pop_front();
        }
    }

    fn get_sample_rate(&self) -> Option<f64> {
        if self.packet_durations.is_empty() {
            return None;
        }
        let total_duration = self.total_duration.as_nanos() as f64 / 1_000_000_000.0;
        let total_size = self.total_size as f64;
        Some(total_size / total_duration)
    }

    fn maybe_display(&mut self, every: Duration) {
        let now = Instant::now();
        if now.duration_since(self.last_display_time) > every {
            eprintln!("{}", self);
            self.last_display_time = now;
        }
    }
}
impl Display for Measurement {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let sample_rate = self.get_sample_rate().unwrap_or(0.0);
        write!(f, "Sample rate: {:.9} Msps", sample_rate / 1_000_000.0)
    }
}

fn open_device_with_vid_pid_timeout(
    context: &Context,
    vid: u16,
    pid: u16,
    timeout: Duration,
) -> Option<rusb::DeviceHandle<rusb::Context>> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(handle) = context.open_device_with_vid_pid(vid, pid) {
            return Some(handle);
        }
    }
    None
}

fn main() {
    let args = Cli::parse();
    let context = Context::new().expect("Could not create USB context");

    if args.firmware.is_some() {
        match context.open_device_with_vid_pid(FX3_VID, FX3_FIRMWARE_PID) {
            Some(handle) => {
                rx888_send_command(&handle, FX3Command::RESETFX3, 0)
                    .expect("Could not reset FX3 to bootloader mode");
            }
            None => {}
        }

        let handle = open_device_with_vid_pid_timeout(
            &context,
            FX3_VID,
            FX3_BOOTLOADER_PID,
            Duration::from_secs(1),
        )
        .expect("Could not find or open bootloader");

        let mut file = File::open(args.firmware.unwrap()).expect("Could not open firmware file");

        fx3::fx3_load_ram(handle, &mut file).expect("Could not load firmware");

        thread::sleep(Duration::from_millis(1000));
    }

    let mut output_file = args.output.map(|path| {
        if path == PathBuf::from("-") {
            Box::new(std::io::stdout()) as Box<dyn Write>
        } else {
            let file = File::open(path).expect("Could not open output file");
            Box::new(file) as Box<dyn Write>
        }
    });

    let mut handle = open_device_with_vid_pid_timeout(
        &context,
        FX3_VID,
        FX3_FIRMWARE_PID,
        Duration::from_secs(1),
    )
    .expect("Could not find or open device, did you forget to specify the firmware?");

    if handle.kernel_driver_active(0).unwrap_or(false) {
        handle
            .detach_kernel_driver(0)
            .expect("Could not detach kernel driver");
    }

    handle
        .claim_interface(0)
        .expect("Could not claim interface");

    let mut gpio = 0;
    if args.dither {
        gpio |= GPIOPin::DITH as u32;
    }
    if args.randomize {
        gpio |= GPIOPin::RANDO as u32;
    }
    if args.bias_hf {
        gpio |= GPIOPin::BIAS_HF as u32;
    }
    if args.bias_vhf {
        gpio |= GPIOPin::BIAS_VHF as u32;
    }
    if args.pga {
        gpio |= GPIOPin::PGA_EN as u32;
    }

    let device_name = handle
        .read_product_string_ascii(
            &handle
                .device()
                .device_descriptor()
                .expect("Could not get device descriptor"),
        )
        .unwrap_or("Unknown".to_string());

    /*
    let device = handle.device();
    let config_descriptor = device
        .active_config_descriptor()
        .expect("Could not get config descriptor");
    let mut endpoint_descriptor = config_descriptor
        .interfaces()
        .next()
        .expect("Could not get interface descriptor")
        .descriptors()
        .next()
        .expect("Could not get endpoint descriptor")
        .endpoint_descriptors();
    let max_packet_size = endpoint_descriptor
        .next()
        .expect("Could not get first endpoint descriptor")
        .max_packet_size();

        */
    let packet_size = 1048576;
    let num_transfers = 128;
    let gain = match args.gain_mode {
        GainMode::High => args.gain | 0x80,
        GainMode::Low => args.gain
    };

    let terminate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let terminate = terminate.clone();
        let res = ctrlc::set_handler(move || {
            terminate.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        if res.is_err() {
            eprintln!("Could not set Ctrl-C handler");
        }
    }
    let mut attenuation = args.attenuation as u32;
    rx888_send_command(&handle, FX3Command::TUNERSTDBY, 0).expect("Could not set tuner standby");

    match args.command {
        Some(Commands::VHF {
            frequency,
            vhf_lna,
            vhf_vga,
            vhf_sideband,
            vhf_harmonic
        }) => {
            gpio |= GPIOPin::VHF_EN as u32;

            rx888_send_command(&handle, FX3Command::TUNERINIT, 0)
                .expect("Could not initialize tuner");
            rx888_send_command_u64(&handle, FX3Command::TUNERTUNE, frequency)
                .expect("Could not tune tuner");
            rx888_send_argument(&handle, ArgumentList::R82XX_ATTENUATOR, vhf_lna as u16)
                .expect("Could not set R82XX_ATTENUATOR");
            rx888_send_argument(&handle, ArgumentList::R82XX_VGA, vhf_vga as u16)
                .expect("Could not set R82XX_VGA");
            rx888_send_argument(&handle, ArgumentList::R82XX_SIDEBAND, vhf_sideband as u16)
                .expect("Could not set R82XX_SIDEBAND");
            rx888_send_argument(&handle, ArgumentList::R82XX_HARMONIC, vhf_harmonic as u16)
                .expect("Could not set R82XX_HARMONIC");

            attenuation = 20;
        }
        None => {}
    }

    if device_name == "RX888" {
        // Different attentuator settings for RX888
        if args.attenuation == 0 {
            gpio |= GPIOPin::ATT_SEL1 as u32;
        } else if args.attenuation == 10 {
            gpio |= GPIOPin::ATT_SEL1 as u32;
            gpio |= GPIOPin::ATT_SEL0 as u32;
        } else if args.attenuation == 20 {
            gpio |= GPIOPin::ATT_SEL0 as u32;
        } else {
            panic!("Invalid attenuation setting, only specify 0, 1 or 2 for RX888 non mk2")
        }
    }
    eprintln!("Attenuation: {}", attenuation);
    eprintln!("Gain: {}", gain);
    rx888_send_command(&handle, FX3Command::GPIOFX3, gpio).expect("Could not set GPIO");
    rx888_send_argument(&handle, ArgumentList::DAT31_ATT, attenuation as u16)
        .expect("Could not set ATT");
    rx888_send_argument(&handle, ArgumentList::AD8340_VGA, gain as u16).expect("Could not set VGA");
    rx888_send_command(&handle, FX3Command::STARTADC, args.sample_rate)
        .expect("Could not start ADC");
    rx888_send_command(&handle, FX3Command::STARTFX3, 0).expect("Could not start FX3");

    let handle = Arc::new(handle);
    let mut transfer_pool =
        TransferPool::new(handle.clone()).expect("Could not create transfer pool");

    while transfer_pool.pending() < num_transfers {
        transfer_pool
            .submit_bulk(0x81, Vec::with_capacity(packet_size))
            .expect("Could not submit transfer");
    }

    let timeout = Duration::from_secs(1);
    let mut measurement = Measurement::new();

    while !terminate.load(std::sync::atomic::Ordering::Relaxed) {
        let mut data = transfer_pool.poll(timeout).expect("Transfer failed");
        if args.randomize {
            let data_u16: &mut [u16] = cast_slice_mut(&mut data);
            for i in 0..data_u16.len() {
                data_u16[i] ^= 0xFFFE * (data_u16[i] & 0x1);
            }
        }
        let _ = output_file.iter_mut().for_each(|file| {
            let _ = file.write_all(&data);
        });
        if args.measure || output_file.is_none() {
            measurement.add_packet(data.len() / 2);
            measurement.maybe_display(Duration::from_secs(1));
        }
        transfer_pool
            .submit_bulk(0x81, data)
            .expect("Failed to resubmit transfer");
    }

    transfer_pool.cancel_all();

    rx888_send_command(handle.as_ref(), FX3Command::STARTADC, 10000000)
        .expect("Could not downclock ADC");
    rx888_send_command(handle.as_ref(), FX3Command::STOPFX3, 0).expect("Could not stop FX3");
}
