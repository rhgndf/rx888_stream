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
use rusb::{Context, DeviceHandle, UsbContext};
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

    /// USB transfer packet size
    #[arg(long, global = true, default_value_t = 131072)]
    packet_size: usize,

    /// Number of USB transfers
    #[arg(long, global = true, default_value_t = 32)]
    num_transfers: usize,
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

/// Stops the FX3's GPIF streaming engine when dropped.
///
/// The capture loop's normal exit sends STOPFX3 explicitly, but a signal
/// (SIGINT/SIGTERM) interrupts the blocking libusb event wait and makes
/// `rusb-async`'s `poll()` panic *from inside the crate* (it `panic!`s on any
/// `libusb_handle_events` error, including EINTR / error -10). A panic skips
/// the explicit shutdown, so without this guard an interrupted capture leaves
/// the FX3 streaming into a dead endpoint — the next run then fails to start
/// it (STARTFX3 timeout / RESETFX3 Io) and needs a physical replug.
///
/// As a Drop guard it runs during the panic unwind too (the build uses the
/// default `panic = "unwind"`), guaranteeing STOPFX3 is sent on every exit
/// path. Errors are ignored: we're tearing down regardless.
struct Fx3StopGuard {
    handle: Arc<DeviceHandle<Context>>,
}

impl Drop for Fx3StopGuard {
    fn drop(&mut self) {
        let _ = rx888_send_command(self.handle.as_ref(), FX3Command::STARTADC, 10_000_000);
        let _ = rx888_send_command(self.handle.as_ref(), FX3Command::STOPFX3, 0);
    }
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
            let file = File::create(path).expect("Could not open output file");
            Box::new(file) as Box<dyn Write>
        }
    });

    let handle = open_device_with_vid_pid_timeout(
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
    let packet_size = args.packet_size;
    let num_transfers = args.num_transfers;
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

            // Pass the R828D's reference-clock frequency (16 MHz) here, not 0.
            // The firmware forwards this value as the `freq` argument to
            // `r820_initialize()`. Without a working reference the R828D PLL
            // produces unpredictable LO frequencies and VHF reception drifts
            // run-to-run with no decodable signal at the requested tune freq.
            // Constant taken from ExtIO_sddc Core/radio/RX888R2Radio.cpp:3:
            //     #define R828D_FREQ (16000000) // R820T reference frequency
            rx888_send_command(&handle, FX3Command::TUNERINIT, 16_000_000)
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
    // Guarantees STOPFX3 on every exit path, including a signal-induced panic
    // inside rusb-async's poll(). See Fx3StopGuard. Declared before the
    // transfer pool so it drops *after* the pool cancels its transfers.
    let _stop_guard = Fx3StopGuard {
        handle: handle.clone(),
    };
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
        let mut data = match transfer_pool.poll(timeout) {
            Ok(data) => data,
            // A signal (SIGINT/SIGTERM) interrupting the blocking libusb event
            // wait can surface here as an error. If the handler has already set
            // `terminate`, this is an orderly shutdown: break to the graceful
            // STOPFX3 below rather than treating it as a fatal transfer error.
            // (The signal can alternatively make poll() panic from inside
            // rusb-async on EINTR — that path is covered by Fx3StopGuard.)
            Err(_) if terminate.load(std::sync::atomic::Ordering::Relaxed) => break,
            Err(e) => panic!("Transfer failed: {e:?}"),
        };
        if args.randomize {
            let data_u16: &mut [u16] = cast_slice_mut(&mut data);
            for i in 0..data_u16.len() {
                data_u16[i] ^= 0xFFFE * (data_u16[i] & 0x1);
            }
        }
        let _ = output_file.iter_mut().for_each(|file| {
            file.write_all(&data).expect("Stream output error")
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
    // STARTADC-downclock + STOPFX3 are sent by Fx3StopGuard's Drop as the
    // scope unwinds — on both the normal break above and a poll() panic.
}
