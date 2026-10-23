#![cfg_attr(windows, windows_subsystem = "windows")]

mod form;
mod human;
mod report;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed2k_vault_core::progress::Progress;
use ed2k_vault_core::scan::{default_jobs, default_threads, scan};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use form::{format_for_path, with_format_extension, Form, FORMATS};

slint::include_modules!();

/// A scan in flight.
struct Run {
    progress: Arc<Progress>,
    cancel: Arc<AtomicBool>,
}

fn read_form(ui: &MainWindow) -> Form {
    Form {
        root: ui.get_root_path().into(),
        output: ui.get_output_path().into(),
        format_index: ui.get_format_index().max(0) as usize,
        hashset: ui.get_hashset(),
        aich: ui.get_aich(),
        hidden: ui.get_hidden(),
        use_cache: ui.get_use_cache(),
        include_ext: ui.get_include_ext().into(),
        exclude_ext: ui.get_exclude_ext().into(),
        exclude: ui.get_exclude().into(),
        exclude_dirs: ui.get_exclude_dirs().into(),
        min_size: ui.get_min_size().into(),
        max_size: ui.get_max_size().into(),
        threads: ui.get_threads().max(1) as usize,
        jobs: ui.get_jobs().max(1) as usize,
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let labels: Vec<SharedString> = FORMATS.iter().map(|f| f.0.into()).collect();
    ui.set_formats(ModelRc::new(VecModel::from(labels)));
    let threads = default_threads();
    ui.set_threads(threads as i32);
    ui.set_jobs(default_jobs(threads) as i32);

    let run: Rc<RefCell<Option<Run>>> = Rc::default();
    // Closing the window mid-scan cancels it first so the output file is finalized.
    let quit_when_done = Arc::new(AtomicBool::new(false));

    ui.window().on_close_requested({
        let ui = ui.as_weak();
        let run = Rc::clone(&run);
        let quit_when_done = Arc::clone(&quit_when_done);
        move || {
            let running = ui.upgrade().is_some_and(|ui| ui.get_running());
            if !running {
                return slint::CloseRequestResponse::HideWindow;
            }
            if let Some(run) = run.borrow().as_ref() {
                run.cancel.store(true, Ordering::Relaxed);
            }
            quit_when_done.store(true, Ordering::Relaxed);
            if let Some(ui) = ui.upgrade() {
                ui.set_status("Cancelling, the window closes when the file is saved…".into());
            }
            slint::CloseRequestResponse::KeepWindowShown
        }
    });

    ui.on_browse_root({
        let ui = ui.as_weak();
        move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                if let Some(ui) = ui.upgrade() {
                    ui.set_root_path(folder.to_string_lossy().as_ref().into());
                }
            }
        }
    });

    ui.on_browse_output({
        let ui = ui.as_weak();
        move || {
            let Some(ui) = ui.upgrade() else { return };
            let index = (ui.get_format_index().max(0) as usize).min(FORMATS.len() - 1);
            let (label, _, extension) = FORMATS[index];
            let picked = rfd::FileDialog::new()
                .add_filter(label, &[extension])
                .set_file_name(format!("ed2k-links.{extension}"))
                .save_file();
            if let Some(path) = picked {
                ui.set_output_path(path.to_string_lossy().as_ref().into());
            }
        }
    });

    ui.on_output_edited({
        let ui = ui.as_weak();
        move |text| {
            if let (Some(ui), Some(index)) = (ui.upgrade(), format_for_path(&text)) {
                ui.set_format_index(index as i32);
            }
        }
    });

    ui.on_format_changed({
        let ui = ui.as_weak();
        move |index| {
            if let Some(ui) = ui.upgrade() {
                let path = with_format_extension(&ui.get_output_path(), index.max(0) as usize);
                ui.set_output_path(path.into());
            }
        }
    });

    ui.on_start({
        let ui = ui.as_weak();
        let run = Rc::clone(&run);
        let quit_when_done = Arc::clone(&quit_when_done);
        move || {
            let Some(ui) = ui.upgrade() else { return };
            ui.set_error("".into());
            let config = match read_form(&ui).to_config() {
                Ok(config) => config,
                Err(message) => {
                    ui.set_error(message.into());
                    return;
                }
            };

            let progress = Arc::new(Progress::default());
            let cancel = Arc::new(AtomicBool::new(false));
            *run.borrow_mut() = Some(Run {
                progress: Arc::clone(&progress),
                cancel: Arc::clone(&cancel),
            });
            ui.set_report("".into());
            ui.set_progress(0.0);
            ui.set_current_file("".into());
            ui.set_status("Reading the folder…".into());
            ui.set_running(true);

            let weak = ui.as_weak();
            let quit_when_done = Arc::clone(&quit_when_done);
            std::thread::spawn(move || {
                let result = scan(&config, &progress, &cancel);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    ui.set_running(false);
                    match result {
                        Ok(summary) => {
                            if !summary.cancelled {
                                ui.set_progress(1.0);
                            }
                            ui.set_status(report::headline(&summary).into());
                            ui.set_report(report::details(&summary).into());
                        }
                        Err(error) => {
                            ui.set_status("".into());
                            ui.set_error(error.to_string().into());
                        }
                    }
                    if quit_when_done.load(Ordering::Relaxed) {
                        let _ = slint::quit_event_loop();
                    }
                });
            });
        }
    });

    ui.on_cancel({
        let ui = ui.as_weak();
        let run = Rc::clone(&run);
        move || {
            if let Some(run) = run.borrow().as_ref() {
                run.cancel.store(true, Ordering::Relaxed);
            }
            if let Some(ui) = ui.upgrade() {
                ui.set_status("Cancelling…".into());
            }
        }
    });

    // Refresh the progress area ten times a second while a scan runs.
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(100), {
        let ui = ui.as_weak();
        let run = Rc::clone(&run);
        let mut last = (Instant::now(), 0u64, 0f64);
        move || {
            let Some(ui) = ui.upgrade() else { return };
            if !ui.get_running() {
                last = (Instant::now(), 0, 0.0);
                return;
            }
            let run = run.borrow();
            let Some(run) = run.as_ref() else { return };
            if run.cancel.load(Ordering::Relaxed) {
                return;
            }
            let snap = run.progress.snapshot();

            let now = Instant::now();
            let elapsed = now.duration_since(last.0).as_secs_f64();
            if elapsed > 0.0 {
                let instant = snap.bytes_read.saturating_sub(last.1) as f64 / elapsed;
                let speed = if last.2 == 0.0 {
                    instant
                } else {
                    last.2 * 0.8 + instant * 0.2
                };
                last = (now, snap.bytes_read, speed);
            }

            let fraction = if snap.walk_finished && snap.bytes_found > 0 {
                snap.bytes_done as f32 / snap.bytes_found as f32
            } else {
                0.0
            };
            ui.set_progress(fraction.min(1.0));
            ui.set_status(report::status(&snap, last.2).into());
            ui.set_current_file(snap.last_file.into());
        }
    });

    ui.run()
}
