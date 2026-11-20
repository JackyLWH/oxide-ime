#![windows_subsystem = "windows"]

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let result = match args.as_slice() {
        [] => oxide_ime::run_dictionary_broker(false),
        [arg] if arg == "--restart-broker" => oxide_ime::run_dictionary_broker(true),
        [arg] if arg == "--check-base" || arg == "--check" => {
            oxide_ime::check_dictionary_broker(None)
        }
        [arg, pid] if arg == "--check" => pid
            .to_str()
            .and_then(|pid| pid.parse::<u32>().ok())
            .filter(|pid| *pid != 0)
            .ok_or_else(|| "無效的字庫服務 PID".to_owned())
            .and_then(|pid| oxide_ime::check_dictionary_broker(Some(pid))),
        #[cfg(feature = "integration-fixtures")]
        [arg, sid] if arg == "--test-allow" || arg == "--test-deny" => {
            oxide_ime::set_test_sandbox_permission(&sid.to_string_lossy(), arg == "--test-allow")
        }
        _ => Err("無效的啟動參數".into()),
    };
    if let Err(error) = result {
        eprintln!("字庫服務：{error}");
        std::process::exit(1);
    }
}
