# Tool to stream data from RX888

Written in rust for
* Better maintainability
* More features

## Building and running
### Build
```
RUSTFLAGS="-C target-cpu=native" cargo build --profile release
```
### Run
This outputs the samples to stdout. 
```
./target/release/rx888_stream -f SDDC_FX3.img --sample-rate 100000000 -r /dev/null -
./target/release/rx888_stream --help # view help
```

