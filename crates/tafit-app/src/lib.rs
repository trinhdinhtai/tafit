//! Startup: parses the command line, builds the store, and hands it to the UI.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use tafit_base::APP_NAME;
use tafit_git_gix::GixBackend;
use tafit_state::{AppStore, Msg, StoreEvents, WindowId};

/// Entry point of the `tafit` binary.
pub fn run() -> ExitCode {
    let path = match repository_arg(std::env::args_os().skip(1)) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let (store, events) = AppStore::new(Arc::new(GixBackend::default()));
    store.dispatch(Msg::DetectGit);
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path,
    });
    run_ui(store, events)
}

/// `tafit [<path>]`: the Repository to open, made absolute; the current
/// directory when omitted.
fn repository_arg(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<PathBuf, String> {
    let usage = format!("usage: {APP_NAME} [<path>]");
    let path = match (args.next(), args.next()) {
        (None, _) => PathBuf::from("."),
        (Some(arg), None) if arg != "-h" && arg != "--help" => PathBuf::from(arg),
        _ => return Err(usage),
    };
    std::path::absolute(&path).map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(feature = "ui-rune")]
fn run_ui(store: AppStore, events: StoreEvents) -> ExitCode {
    tafit_ui_rune::run(Arc::new(store), events);
    ExitCode::SUCCESS
}

#[cfg(all(feature = "ui-gpui", not(feature = "ui-rune")))]
fn run_ui(store: AppStore, events: StoreEvents) -> ExitCode {
    tafit_ui_gpui::run(Arc::new(store), events);
    ExitCode::SUCCESS
}

#[cfg(not(any(feature = "ui-gpui", feature = "ui-rune")))]
fn run_ui(_store: AppStore, _events: StoreEvents) -> ExitCode {
    eprintln!("{APP_NAME} was built without a UI (feature `ui-rune`)");
    ExitCode::FAILURE
}
