#[global_allocator]
static ALLOC: rusty_alloc::RustyAlloc = rusty_alloc::RustyAlloc;

fn main() {
    chuzz::install_crypto_provider();
    if let Err(error) = chuzz::serve::reactor_witness() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
