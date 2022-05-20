#ifndef AEED665A_44C2_4A05_B7F0_159B1C39787C
#define AEED665A_44C2_4A05_B7F0_159B1C39787C

#include "libusb.h"
#include <stdint.h>

enum FX3Command {
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
};

int command_send(struct libusb_device_handle *dev_handle, enum FX3Command cmd,
                 uint32_t data);
int ezusb_upload_firmware(libusb_device *dev, int configuration,
                          const char *name);

#endif /* AEED665A_44C2_4A05_B7F0_159B1C39787C */
