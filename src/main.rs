#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clock;
mod detect;
mod platform;
mod fault;
mod lang;
mod net;
mod pet;
mod render;
mod roam;
mod talk;
mod tools;
mod trace;
mod tuning;
mod update;
mod watch;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // What the menu asks for when it needs an administrator: the task the
    // package registered, turned on or off. The exit code is the answer.
    if let Some(on) = args.iter().find_map(|a| match a.as_str() {
        "--enable-task" => Some(true),
        "--disable-task" => Some(false),
        _ => None,
    }) {
        std::process::exit(if platform::autostart::enable_task(on) { 0 } else { 1 });
    }
    // The package stops him before it writes over him or takes him away.
    if args.iter().any(|a| a == "--stop") {
        platform::autostart::end_task();
        platform::system::close_running();
        platform::net::stop_kernel_trace();
        return;
    }
    // Asked by the package on its way out, as the user rather than as the
    // installer, since the memory is that user's and nobody else's.
    if args.iter().any(|a| a == "--ask-forget") {
        if platform::system::ask(lang::Lang::detect().forget_question()) {
            let _ = std::fs::remove_dir_all(platform::host::app_dir());
        }
        return;
    }
    fault::install_hook();
    platform::system::wait_for_predecessor();
    app::run();
}
