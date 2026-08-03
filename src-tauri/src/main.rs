// Windows release 下不弹出额外的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args_os();
    let _ = args.next();
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--helm-runtime-hook")) {
        let state = args
            .next()
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        std::process::exit(helm_lib::claude_permission_hook::run_native_runtime_hook(
            &state,
        ));
    }
    helm_lib::run();
}
