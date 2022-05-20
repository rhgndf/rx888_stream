

#include "ezusb.h"
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "libusb.h"
#include <stdio.h>

/**
 * Read a 32 bits little endian unsigned integer out of memory.
 * @param x a pointer to the input memory
 * @return the corresponding unsigned integer
 */
#define RL32(x)                                                                \
  (((unsigned)((const uint8_t *)(x))[3] << 24) |                               \
   ((unsigned)((const uint8_t *)(x))[2] << 16) |                               \
   ((unsigned)((const uint8_t *)(x))[1] << 8) |                                \
   (unsigned)((const uint8_t *)(x))[0])

#undef MIN
#define MIN(a, b) (((a) < (b)) ? (a) : (b))

#define FW_CHUNKSIZE (4 * 1024)

/**
 * Retrieve the size of the open stream @a file.
 *
 * This function only works on seekable streams. However, the set of seekable
 * streams is generally congruent with the set of streams that have a size.
 * Code that needs to work with any type of stream (including pipes) should
 * require neither seekability nor advance knowledge of the size.
 * On failure, the return value is negative and errno is set.
 *
 * @param file An I/O stream opened in binary mode.
 * @return The size of @a file in bytes, or a negative value on failure.
 *
 * @private
 */
int64_t file_get_size(FILE *file) {
  off_t filepos, filesize;

  /* ftello() and fseeko() are not standard C, but part of POSIX.1-2001.
   * Thus, if these functions are available at all, they can reasonably
   * be expected to also conform to POSIX semantics. In particular, this
   * means that ftello() after fseeko(..., SEEK_END) has a defined result
   * and can be used to get the size of a seekable stream.
   * On Windows, the result is fully defined only for binary streams.
   */
  filepos = ftello(file);
  if (filepos < 0)
    return -1;

  if (fseeko(file, 0, SEEK_END) < 0)
    return -1;

  filesize = ftello(file);
  if (filesize < 0)
    return -1;

  if (fseeko(file, filepos, SEEK_SET) < 0)
    return -1;

  return filesize;
}

int ezusb_install_firmware(libusb_device_handle *hdl, const char *filename) {
  unsigned char *firmware;
  void *buf;
  size_t n_read, length, offset, chunksize;
  int ret, result;
  FILE *file = NULL;

  int64_t filesize;
  const size_t max_size = 0x86000;

  file = fopen(filename, "rb");

  if (file)
    fprintf(stderr, "Opened '%s'.\n", filename);
  else {
    fprintf(stderr, "Attempt to open file failed\n");

    free(file);
  }

  if (!file) {
    fprintf(stderr, "Failed to locate '%s'.\n", filename);
    return -1;
  }

  filesize = file_get_size(file);
  if (filesize < 0) {
    fprintf(stderr, "Failed to obtain size of '%s'\n", filename);
    fclose(file);
    return -1;
  }

  if (filesize > max_size) {
    fprintf(stderr, "Size %" PRIu64 " of '%s' exceeds limit %zu.\n", filesize,
            filename, max_size);
    if (fclose(file) < 0)
      fprintf(stderr, "Failed to close file\n");
    return -1;
  }

  buf = malloc(filesize);
  if (!buf) {
    fprintf(stderr, "Failed to allocate buffer for '%s'.\n", filename);
    return -1;
  }

  n_read = fread(buf, 1, filesize, file);
  if (fclose(file) < 0) {
    fprintf(stderr, "Failed to close file\n");
    return -1;
  }

  if (n_read < 0 || (size_t)n_read != filesize) {
    if (n_read >= 0)
      fprintf(stderr, "Failed to read '%s': premature end of file.\n",
              filename);
    free(buf);
    return -1;
  }
  firmware = buf;

  length = filesize;

  if (!firmware)
    return -1;

  fprintf(stderr, "Uploading firmware '%s'.\n", filename);

  result = 0;
  offset = 0;

  if (length < 4 || firmware[0] != 'C' || firmware[1] != 'Y' ||
      firmware[3] != 0xb0) {
    fprintf(stderr, "Invalid signature on firmware\n");
    free(firmware);
    return -1;
  }
  offset = 4;

  while (offset < length) {
    size_t addr, sublength, suboffset;

    if (offset + 4 == length) {
      /* Skip checksum */
      offset += 4;
      break;
    }
    if (length < offset + 8) {
      break;
    }
    sublength = RL32(firmware + offset) << 2;
    offset += 4;
    addr = RL32(firmware + offset);
    offset += 4;
    if (sublength > length - offset) {
      break;
    }

    suboffset = 0;

    do {
      chunksize = MIN(sublength - suboffset, FW_CHUNKSIZE);

      ret = libusb_control_transfer(
          hdl, LIBUSB_REQUEST_TYPE_VENDOR | LIBUSB_ENDPOINT_OUT, 0xa0,
          (addr + suboffset) & 0xffff, (addr + suboffset) >> 16,
          firmware + offset + suboffset, chunksize, 100);
      if (ret < 0) {
        fprintf(stderr, "Unable to send firmware to device: %s.\n",
                libusb_error_name(ret));
        free(firmware);
        return -1;
      }
      fprintf(stderr, "Uploaded %zu bytes.\n", chunksize);
      suboffset += chunksize;
    } while (suboffset < sublength);

    offset += sublength;
  }
  free(firmware);

  if (offset < length) {
    fprintf(stderr, "Firmware file is truncated.\n");
    return -1;
  }

  fprintf(stderr, "Firmware upload done.\n");

  return result;
}

int ezusb_reset(struct libusb_device_handle *hdl, int set_clear) {
  int ret;
  unsigned char buf[1];

  fprintf(stderr, "setting CPU reset mode %s...\n", set_clear ? "on" : "off");
  buf[0] = set_clear ? 1 : 0;
  ret = libusb_control_transfer(hdl, LIBUSB_REQUEST_TYPE_VENDOR, 0xa0, 0xe600,
                                0x0000, buf, 1, 100);
  if (ret < 0)
    fprintf(stderr, "Unable to send control request: %s.\n",
            libusb_error_name(ret));

  return ret;
}

int ezusb_upload_firmware(libusb_device *dev, int configuration,
                          const char *filename) {
  struct libusb_device_handle *hdl;
  int ret;

  fprintf(stderr, "Uploading firmware to device on %d.%d\n",
          libusb_get_bus_number(dev), libusb_get_device_address(dev));

  if ((ret = libusb_open(dev, &hdl)) < 0) {
    fprintf(stderr, "failed to open device: %s.\n", libusb_error_name(ret));
    return -1;
  }

/*
 * The libusb Darwin backend is broken: it can report a kernel driver being
 * active, but detaching it always returns an error.
 */
#if !defined(__APPLE__)
  if (libusb_kernel_driver_active(hdl, 0) == 1) {
    if ((ret = libusb_detach_kernel_driver(hdl, 0)) < 0) {
      fprintf(stderr, "failed to detach kernel driver: %s\n",
              libusb_error_name(ret));
      return -1;
    }
  }
#endif

  if ((ret = libusb_set_configuration(hdl, configuration)) < 0) {
    fprintf(stderr, "Unable to set configuration: %s\n",
            libusb_error_name(ret));
    return -1;
  }

  if (ezusb_install_firmware(hdl, filename) < 0)
    return -1;

  libusb_close(hdl);

  return 0;
}

/**
 * Check the USB configuration to determine if this device has a given
 * manufacturer and product string.
 *
 * @return TRUE if the device's configuration profile strings
 *         configuration, FALSE otherwise.
 */
int usb_match_manuf_prod(libusb_device *dev, const char *manufacturer,
                         const char *product) {
  struct libusb_device_descriptor des;
  struct libusb_device_handle *hdl;
  int ret;
  unsigned char strdesc[64];

  hdl = NULL;
  ret = false;
  while (!ret) {
    /* Assume the FW has not been loaded, unless proven wrong. */
    libusb_get_device_descriptor(dev, &des);

    if (libusb_open(dev, &hdl) != 0)
      break;

    if (libusb_get_string_descriptor_ascii(hdl, des.iManufacturer, strdesc,
                                           sizeof(strdesc)) < 0)
      break;
    if (strcmp((const char *)strdesc, manufacturer))
      break;

    if (libusb_get_string_descriptor_ascii(hdl, des.iProduct, strdesc,
                                           sizeof(strdesc)) < 0)
      break;
    if (strcmp((const char *)strdesc, product))
      break;

    ret = true;
  }
  if (hdl)
    libusb_close(hdl);

  return ret;
}

int command_send(struct libusb_device_handle *dev_handle, enum FX3Command cmd,
                 uint32_t data) {

  int ret;

  /* Send the control message. */
  ret = libusb_control_transfer(
      dev_handle, LIBUSB_REQUEST_TYPE_VENDOR | LIBUSB_ENDPOINT_OUT, cmd, 0, 0,
      (unsigned char *)&data, sizeof(data), 0);

  if (ret < 0) {
    fprintf(stderr, "Could not send command: 0x%X with data: %d. Error : %s.\n",
            cmd, data, libusb_error_name(ret));
    return -1;
  }

  return 0;
}
