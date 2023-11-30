use std::time::Duration;

use rusb::{
    constants::{LIBUSB_ENDPOINT_OUT, LIBUSB_REQUEST_TYPE_VENDOR},
    Context, DeviceHandle,
};

#[allow(dead_code)]
pub enum FX3Command {
    // Start GPII engine and stream the data from ADC
    // WRITE: UINT32
    STARTFX3 = 0xAA,

    // Stop GPII engine
    // WRITE: UINT32
    STOPFX3 = 0xAB,

    // Get the information of device
    // including model, version
    // READ: UINT32
    TESTFX3 = 0xAC,

    // Control GPIOs
    // WRITE: UINT32
    GPIOFX3 = 0xAD,

    // Write data to I2c bus
    // WRITE: DATA
    // INDEX: reg
    // VALUE: i2c_addr
    I2CWFX3 = 0xAE,

    // Read data from I2c bus
    // READ: DATA
    // INDEX: reg
    // VALUE: i2c_addr
    I2CRFX3 = 0xAF,

    // Reset USB chip and get back to bootloader mode
    // WRITE: NONE
    RESETFX3 = 0xB1,

    // Set Argument, packet Index/Vaule contains the data
    // WRITE: (Additional Data)
    // INDEX: Argument_index
    // VALUE: arguement value
    SETARGFX3 = 0xB6,

    // Start ADC with the specific frequency
    // Optional, if ADC is running with crystal, this is not needed.
    // WRITE: UINT32 -> adc frequency
    STARTADC = 0xB2,

    // R82XX family Tuner functions
    // Initialize R82XX tuner
    // WRITE: NONE
    TUNERINIT = 0xB4,

    // Tune to a sepcific frequency
    // WRITE: UINT64
    TUNERTUNE = 0xB5,

    // Stop Tuner
    // WRITE: NONE
    TUNERSTDBY = 0xB8,

    // Read Debug string if any
    // READ:
    READINFODEBUG = 0xBA,
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub enum ArgumentList {
    // Set R8xx lna/mixer gain
    // value: 0-29
    R82XX_ATTENUATOR = 1,

    // Set R8xx vga gain
    // value: 0-15
    R82XX_VGA = 2,

    // Set R8xx sideband
    // value: 0/1
    R82XX_SIDEBAND = 3,

    // Set R8xx harmonic
    // value: 0/1
    R82XX_HARMONIC = 4,

    // Set DAT-31 Att
    // Value: 0-63
    DAT31_ATT = 10,

    // Set AD8340 chip vga
    // Value: 0-255
    AD8340_VGA = 11,

    // Preselector
    // Value: 0-2
    PRESELECTOR = 12,

    // VHFATT
    // Value: 0-15
    VHF_ATTENUATOR = 13,
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub enum GPIOPin {
    ATT_LE = 1 << 0,
    ATT_CLK = 1 << 1,
    ATT_DATA = 1 << 2,
    SEL0 = 1 << 3,
    SEL1 = 1 << 4,
    SHDWN = 1 << 5,
    DITH = 1 << 6,
    RANDO = 1 << 7,
    BIAS_HF = 1 << 8,
    BIAS_VHF = 1 << 9,
    LED_YELLOW = 1 << 10,
    LED_RED = 1 << 11,
    LED_BLUE = 1 << 12,
    ATT_SEL0 = 1 << 13,
    ATT_SEL1 = 1 << 14,

    // RX888r2
    VHF_EN = 1 << 15,
    PGA_EN = 1 << 16,
}

pub fn rx888_send_command(
    handle: &DeviceHandle<Context>,
    cmd: FX3Command,
    data: u32,
) -> rusb::Result<usize> {
    let timeout = Duration::from_secs(1);

    handle.write_control(
        LIBUSB_ENDPOINT_OUT | LIBUSB_REQUEST_TYPE_VENDOR,
        cmd as u8,
        0,
        0,
        &data.to_le_bytes(),
        timeout,
    )
}

pub fn rx888_send_argument(
    handle: &DeviceHandle<Context>,
    cmd: ArgumentList,
    data: u32,
) -> rusb::Result<usize> {
    let timeout = Duration::from_secs(1);

    handle.write_control(
        LIBUSB_REQUEST_TYPE_VENDOR | LIBUSB_ENDPOINT_OUT,
        FX3Command::SETARGFX3 as u8,
        data as u16,
        cmd as u16,
        &[0],
        timeout,
    )
}
