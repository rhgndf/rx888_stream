mod ezusb;
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
use clap::{value_parser, Parser, ValueEnum};
use rusb::{Context, UsbContext};
use rusb_async::TransferPool;
use rx888::{rx888_send_argument, rx888_send_command, ArgumentList, FX3Command, GPIOPin};

const FX3_VID: u16 = 0x04b4;
const FX3_BOOTLOADER_PID: u16 = 0x00f3;
const FX3_FIRMWARE_PID: u16 = 0x00f1;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum GainMode {
    /// High gain mode
    High,
    /// Low gain mode
    Low,
}

/// RX888 USB streamer program
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Firmware file to load
    #[arg(short, long)]
    firmware: Option<PathBuf>,

    /// Enable dithering
    #[arg(short, long, default_value_t = false)]
    dither: bool,

    /// Enable randomization
    #[arg(short, long, default_value_t = false)]
    randomize: bool,

    /// ADC sample rate
    #[arg(short, long, default_value_t = 50000000, value_parser = value_parser!(u32).range(10000000..150000000))]
    sample_rate: u32,

    /// VGA gain setting 0-127
    #[arg(short, long, default_value_t = 1, value_parser = value_parser!(u8).range(0..=127))]
    gain: u8,

    /// VGA gain mode high or low
    #[arg(short = 'm', long, default_value = "high")]
    gain_mode: GainMode,

    /// Attenuator setting 0-63
    #[arg(short, long, default_value_t = 0, value_parser = value_parser!(u8).range(0..=63))]
    attenuation: u8,

    /// HF Bias-T
    #[arg(long, default_value_t = false)]
    bias_hf: bool,

    /// VHF Bias-T
    #[arg(long, default_value_t = false)]
    bias_vhf: bool,

    /// PGA enable
    #[arg(long, default_value_t = false)]
    pga: bool,

    /// Output file, "-" is stdout
    output: Option<PathBuf>,

    /// Measurement mode, measures the ADC sample rate
    #[arg(long, default_value_t = false)]
    measure: bool,
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

fn main() {
    let args = Args::parse();
    let context = Context::new().expect("Could not create USB context");

    if args.firmware.is_some() {
        match context.open_device_with_vid_pid(FX3_VID, FX3_FIRMWARE_PID) {
            Some(handle) => {
                rx888_send_command(&handle, FX3Command::RESETFX3, 0)
                    .expect("Could not reset FX3 to bootloader mode");
                thread::sleep(Duration::from_millis(1000));
            }
            None => {}
        }

        let handle = context
            .open_device_with_vid_pid(FX3_VID, FX3_BOOTLOADER_PID)
            .expect("Could not find or open bootloader");

        let mut file = File::open(args.firmware.unwrap()).expect("Could not open firmware file");

        ezusb::fx3_load_ram(handle, &mut file).expect("Could not load firmware");

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

    let mut handle = context
        .open_device_with_vid_pid(FX3_VID, FX3_FIRMWARE_PID)
        .expect("Could not find or open device, did you forget to specify the firmware?");

    if handle.kernel_driver_active(0).unwrap_or(false) {
        handle
            .detach_kernel_driver(0)
            .expect("Could not detach kernel driver");
    }

    handle
        .claim_interface(0)
        .expect("Could not claim interface");

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
    let packet_size = 131072;
    let num_transfers = 32;
    let gain = match args.gain_mode {
        GainMode::High => args.gain,
        GainMode::Low => args.gain | 0x80,
    };
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

    rx888_send_command(&handle, FX3Command::GPIOFX3, gpio).expect("Could not set GPIO");
    rx888_send_argument(&handle, ArgumentList::DAT31_ATT, 0).expect("Could not set ATT");
    rx888_send_argument(&handle, ArgumentList::AD8340_VGA, gain as u32).expect("Could not set VGA");
    rx888_send_command(&handle, FX3Command::STARTADC, args.sample_rate)
        .expect("Could not start ADC");
    rx888_send_command(&handle, FX3Command::STARTFX3, 0).expect("Could not start FX3");
    rx888_send_command(&handle, FX3Command::TUNERSTDBY, 0).expect("Could not set tuner standby");

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

    rx888_send_command(handle.as_ref(), FX3Command::STARTADC, 10000000).expect("Could not downclock ADC");
    rx888_send_command(handle.as_ref(), FX3Command::STOPFX3, 0).expect("Could not stop FX3");
}
