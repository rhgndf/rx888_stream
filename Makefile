

all:
	cc my_usb_example.c ezusb.c -o my_usb_example -g -O0 -fstack-protector-all `pkg-config --cflags --libs libusb-1.0`

clean:
	rm my_usb_example