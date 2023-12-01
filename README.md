# Tool to stream data from RX888

Written in rust for
* Better maintainability
* More features

## Building and running
### Build
```
r
```
### Install
```
RUSTFLAGS="-C target-cpu=native" cargo install --path .
```
### Run
This outputs the samples to stdout. 
```
# HF (default)
./target/release/rx888_stream -f SDDC_FX3.img -r --sample-rate 100000000 -o -
# VHF
./target/release/rx888_stream vhf -f SDDC_FX3.img -r --frequency 145000000 --sample-rate 100000000 -o -
# View help
./target/release/rx888_stream --help
```

