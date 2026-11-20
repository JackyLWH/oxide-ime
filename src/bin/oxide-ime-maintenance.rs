#![windows_subsystem = "windows"]

fn main() {
    if let Err(error) = oxide_ime::run_maintenance() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
