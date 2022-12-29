

all:
	cc rx888_stream.c ezusb.c -o rx888_stream -ggdb3 -O3 -march=native -Wall -Werror -fstack-protector-all `pkg-config --cflags --libs libusb-1.0`

clean:
	rm rx888_stream