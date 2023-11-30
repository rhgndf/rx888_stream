use std::{
    io::{self, Read},
    num::Wrapping,
    time::Duration,
};

use debug_print::debug_eprintln;
use rusb::{
    constants::{
        LIBUSB_ENDPOINT_IN, LIBUSB_ENDPOINT_OUT, LIBUSB_RECIPIENT_DEVICE,
        LIBUSB_REQUEST_TYPE_VENDOR,
    },
    Context, DeviceHandle,
};

const RW_INTERNAL: u8 = 0xA0;

pub fn fx3_load_ram<T: Read>(handle: DeviceHandle<Context>, ram: &mut T) -> io::Result<()> {
    let mut header = [0; 4];
    ram.read_exact(&mut header)?;

    if header[0] != b'C' || header[1] != b'Y' {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid header"));
    }

    if header[3] != 0xB0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported image type",
        ));
    }

    let mut checksum: Wrapping<u32> = Wrapping(0);
    let timeout = Duration::from_secs(1);

    let jump_address = loop {
        let length = {
            let mut buf = [0; 4];
            ram.read_exact(&mut buf)?;
            u32::from_le_bytes(buf)
        };
        let address = {
            let mut buf = [0; 4];
            ram.read_exact(&mut buf)?;
            u32::from_le_bytes(buf)
        };

        if length == 0 {
            break address;
        }

        debug_eprintln!("Loading {} bytes to address {:08x}", length * 4, address);

        let mut data = vec![0; (length as usize) * 4];
        ram.read_exact(&mut data)?;

        checksum += data
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .map(Wrapping)
            .sum::<Wrapping<u32>>();

        data.chunks(4096)
            .enumerate()
            .try_for_each(|(offset, chunk)| -> io::Result<()> {
                let addr = address + offset as u32 * 4096;
                let mut readback_data = [0; 4096];
                handle
                    .write_control(
                        LIBUSB_ENDPOINT_OUT | LIBUSB_REQUEST_TYPE_VENDOR | LIBUSB_RECIPIENT_DEVICE,
                        RW_INTERNAL,
                        (addr & 0xFFFF) as u16,
                        (addr >> 16) as u16,
                        chunk,
                        timeout,
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                handle
                    .read_control(
                        LIBUSB_ENDPOINT_IN | LIBUSB_REQUEST_TYPE_VENDOR | LIBUSB_RECIPIENT_DEVICE,
                        RW_INTERNAL,
                        (addr & 0xFFFF) as u16,
                        (addr >> 16) as u16,
                        &mut readback_data,
                        timeout,
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

                debug_eprintln!("Loading {} bytes to address {:08x}", chunk.len(), addr);
                if chunk != &readback_data[..chunk.len()] {
                    debug_eprintln!("Data mismatch {}", offset);
                    return Err(io::Error::new(io::ErrorKind::Other, "Data mismatch"));
                }
                Ok(())
            })?;
    };

    debug_eprintln!("Jump address: {:08x}", jump_address);

    let firmware_checksum = {
        let mut buf = [0; 4];
        ram.read_exact(&mut buf)?;
        u32::from_le_bytes(buf)
    };

    debug_eprintln!(
        "Checksum: {:08x} Expected checksum: {:08x}",
        checksum.0,
        firmware_checksum
    );
    if checksum.0 != firmware_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Checksum mismatch",
        ));
    }

    handle
        .write_control(
            LIBUSB_ENDPOINT_OUT | LIBUSB_REQUEST_TYPE_VENDOR | LIBUSB_RECIPIENT_DEVICE,
            RW_INTERNAL,
            (jump_address & 0xFFFF) as u16,
            (jump_address >> 16) as u16,
            &[],
            timeout,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(())
}
