/*

Copyright (c)  2021 Ruslan Migirov <trapi78@gmail.com>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

*/

#include "ezusb.h"
#include <libusb.h>
#include <signal.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/time.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

//#define HAS_FIRMWARE true

unsigned int queuedepth = 16; // Number of requests to queue
unsigned int reqsize = 8;     // Request size in number of packets
unsigned int duration = 100;  // Duration of the test in seconds

const char *firmware;

static unsigned int ep = 1 | LIBUSB_ENDPOINT_IN;

static int interface_number = 0;
static struct libusb_device_handle *dev_handle = NULL;
static struct timeval tv_start, tv_end;
unsigned int pktsize;
unsigned int success_count = 0;  // Number of successful transfers
unsigned int failure_count = 0;  // Number of failed transfers
unsigned int transfer_size = 0;  // Size of data transfers performed so far
unsigned int transfer_index = 0; // Write index into the transfer_size array
volatile bool stop_transfers = false; // Request to stop data transfers
volatile int xfers_in_progress = 0;

volatile int sleep_time = 0;

static void transfer_callback(struct libusb_transfer *transfer) {
  unsigned int elapsed_time;
  int size = 0;
  double rate;

  xfers_in_progress--;

  if (transfer->status != LIBUSB_TRANSFER_COMPLETED) {
    failure_count++;
    printf("Transfer callback status %s received %d \
	   bytes.\n",
           libusb_error_name(transfer->status), transfer->actual_length);
  } else {
    size = transfer->actual_length;
    success_count++;
  }
  transfer_size += size;
  transfer_index++;
  if (transfer_index == queuedepth) {
    gettimeofday(&tv_end, NULL);
    elapsed_time = ((tv_end.tv_sec - tv_start.tv_sec) * 1000000 +
                    (tv_end.tv_usec - tv_start.tv_usec));
    printf("Transfer Counts: %d pass %d fail. %d per pass\n", success_count,
           failure_count, transfer->actual_length);
    rate = ((double)transfer_size / 1024) / ((double)elapsed_time / 1000000);
    printf("Data Rate: %d KBps\n\n", (uint32_t)rate);
    transfer_index = 0;
    transfer_size = 0;
    tv_start = tv_end;
  }

  if (!stop_transfers) {
    if (libusb_submit_transfer(transfer) == 0)
      xfers_in_progress++;
  }
}

// Function to free data buffers and transfer structures
static void free_transfer_buffers(unsigned char **databuffers,
                                  struct libusb_transfer **transfers) {
  // Free up any allocated data buffers
  if (databuffers != NULL) {
    for (unsigned int i = 0; i < queuedepth; i++) {
      if (databuffers[i] != NULL) {
        free(databuffers[i]);
      }
      databuffers[i] = NULL;
    }
    free(databuffers);
  }

  // Free up any allocated transfer structures
  if (transfers != NULL) {
    for (unsigned int i = 0; i < queuedepth; i++) {
      if (transfers[i] != NULL) {
        libusb_free_transfer(transfers[i]);
      }
      transfers[i] = NULL;
    }
    free(transfers);
  }
}

static void sig_hdlr(int signum) {

  (void)signum;
  fprintf(stderr, "\nAbort. Stopping transfers\n");
  stop_transfers = true;
}

int main(int argc, char const *argv[]) {
  /* code */
  struct libusb_device_descriptor desc;
  struct libusb_device *dev;
  struct libusb_endpoint_descriptor const *endpointDesc;
  struct libusb_ss_endpoint_companion_descriptor *ep_comp;
  struct libusb_config_descriptor *config;
  struct libusb_interface_descriptor const *interfaceDesc;
  int ret;
  int if_numsettings;
  int rStatus;
  int64_t fw_updated;
  struct timespec tp;

  uint16_t vendor_id;  //= 0x04b4;
  uint16_t product_id; // = 0x00f1;

  struct sigaction sigact;

  sigact.sa_handler = sig_hdlr;
  sigemptyset(&sigact.sa_mask);
  sigact.sa_flags = 0;
  (void)sigaction(SIGINT, &sigact, NULL);

  struct libusb_transfer **transfers = NULL; // List of transfer structures.
  unsigned char **databuffers = NULL;        // List of data buffers.

  struct timeval t1, t2, tv; // Timestamps used for test duration control
  long sec, usec;

  // if (argc == 2) {
  //    product_id = 0x00f3; //image file in arg,so upload it later
  // } else {
  //   //fprintf(stderr, "Please specify firmware image file in argument");
  //   //exit(1);
  // }

  ret = libusb_init(NULL);
  if (ret != 0) {
    fprintf(stderr, "Error initializing libusb: %s\n", libusb_error_name(ret));
    exit(1);
  }

#if 0
  ret = libusb_kernel_driver_active(dev_handle, 0);
  if (ret != 0) {
    fprintf(stderr, "Kernel driver active. Trying to detach kernel driver\n");
    ret = libusb_detach_kernel_driver(dev_handle, 0);
    if (ret != 0) {
      fprintf(stderr, "Could not detach kernel driver from an interface\n");
      goto close;
    }
  }
#endif
  vendor_id = 0x04b4;

  if (argv[1]) { // there is argument with image file
    firmware = argv[1];
    vendor_id = 0x04b4;
    product_id = 0x00f3;
    // no firmware. upload the firmware
    dev_handle = libusb_open_device_with_vid_pid(NULL, vendor_id, product_id);
    if (!dev_handle) {
      fprintf(stderr, "Error or device could not be found\n");
      goto close;
    }

    dev = libusb_get_device(dev_handle);

    if (ezusb_upload_firmware(dev, 1, firmware) == 0) {
      fw_updated = clock_gettime(CLOCK_MONOTONIC, &tp);
    } else {
      fprintf(stderr,
              "Firmware upload failed for "
              "device %d.%d (logical).\n",
              libusb_get_bus_number(dev), libusb_get_device_address(dev));
    }

    sleep(2);
  }

  product_id = 0x00f1;
  dev_handle = libusb_open_device_with_vid_pid(NULL, vendor_id, product_id);
  if (!dev_handle) {
    fprintf(stderr, "Error or device could not be found\n");
    goto close;
  }

  dev = libusb_get_device(dev_handle);

  libusb_get_config_descriptor(dev, 0, &config);

  ret = libusb_claim_interface(dev_handle, interface_number);
  if (ret != 0) {
    fprintf(stderr, "Error claiming interface\n");
    goto end;
  }

  fprintf(stderr, "Successfully claimed interface\n");

  interfaceDesc = &(config->interface[0].altsetting[0]);

  endpointDesc = &interfaceDesc->endpoint[0];

  libusb_get_device_descriptor(dev, &desc);

  libusb_get_ss_endpoint_companion_descriptor(NULL, endpointDesc, &ep_comp);

  pktsize = endpointDesc->wMaxPacketSize * (ep_comp->bMaxBurst + 1);

  libusb_free_ss_endpoint_companion_descriptor(ep_comp);

  bool allocfail = false;
  databuffers = (u_char **)calloc(queuedepth, sizeof(u_char *));

  transfers = (struct libusb_transfer **)calloc(
      queuedepth, sizeof(struct libusb_transfer *));

  if ((databuffers != NULL) && (transfers != NULL)) {
    for (unsigned int i = 0; i < queuedepth; i++) {
      databuffers[i] = (u_char *)malloc(reqsize * pktsize);
      transfers[i] = libusb_alloc_transfer(0);
      if ((databuffers[i] == NULL) || (transfers[i] == NULL)) {
        allocfail = true;
        break;
      }
    }

  } else {
    allocfail = true;
  }

  if (allocfail) {
    fprintf(stderr, "Failed to allocate buffers and transfers\n");
    free_transfer_buffers(databuffers, transfers);
  }

  gettimeofday(&tv_start, NULL);

  for (unsigned int i = 0; i < queuedepth; i++) {
    libusb_fill_bulk_transfer(transfers[i], dev_handle, ep, databuffers[i],
                              reqsize * pktsize, transfer_callback,
                              (void *)&pktsize, 0);
    rStatus = libusb_submit_transfer(transfers[i]);
    if (rStatus == 0)
      xfers_in_progress++;
  }

  int samplerate = 150 * 1000 * 1000;
  /******/
  command_send(dev_handle, STARTADC, samplerate);
  // usleep(5000);
  command_send(dev_handle, STARTFX3, 0);

  /*******/

  do {
    libusb_handle_events(NULL);

  } while (stop_transfers != true);

  fprintf(stderr, "Test complete. Stopping transfers\n");
  stop_transfers = true;

  while (xfers_in_progress != 0) {
    fprintf(stderr, "%d transfers are pending\n", xfers_in_progress);
    libusb_handle_events(NULL);
    sleep(1);
  }

  fprintf(stderr, "Transfers completed\n");
  free_transfer_buffers(databuffers, transfers);

  command_send(dev_handle, STOPFX3, 0);

end:
  if (dev_handle) {
    libusb_release_interface(dev_handle, interface_number);
  }

  if (config) {
    libusb_free_config_descriptor(config);
  }
close:
  if (dev_handle) {
    libusb_close(dev_handle);
  }
  libusb_exit(NULL);

  return 0;
}
