use bvault_transfer::TransferProgress;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::sync::Mutex;

pub struct ExportProgress {
    multi: MultiProgress,
    main_pb: ProgressBar,
    file_pbs: Mutex<HashMap<String, ProgressBar>>,
}

impl ExportProgress {
    pub fn new() -> Self {
        let multi = MultiProgress::new();
        let main_pb = multi.add(ProgressBar::new(100));
        main_pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        Self {
            multi,
            main_pb,
            file_pbs: Mutex::new(HashMap::new()),
        }
    }
}

impl TransferProgress for ExportProgress {
    fn on_start(&self, _total_files: usize, total_bytes: u64) {
        self.main_pb.set_length(total_bytes);
    }

    fn on_file_start(&self, usb_path: &str, size: u64) {
        let pb = self.multi.add(ProgressBar::new(size));
        pb.set_style(
            ProgressStyle::default_bar()
                .template(&format!(
                    "{{spinner:.green}} {}: [{{bar:20.yellow/white}}] {{bytes}}/{{total_bytes}}",
                    usb_path
                ))
                .unwrap()
                .progress_chars("=>-"),
        );
        let mut map = self.file_pbs.lock().unwrap();
        map.insert(usb_path.to_string(), pb);
    }

    fn on_file_progress(&self, bytes: u64) {
        self.main_pb.inc(bytes);
        // Note: we'd need to thread through the path to update the file pb,
        // but for now updating the main one is sufficient for overall progress
    }

    fn on_file_done(&self, usb_path: &str) {
        let mut map = self.file_pbs.lock().unwrap();
        if let Some(pb) = map.remove(usb_path) {
            pb.finish_and_clear();
        }
    }

    fn on_file_skipped(&self, usb_path: &str) {
        self.multi
            .println(format!("✓ Skipped {}", usb_path))
            .unwrap();
    }

    fn on_error(&self, usb_path: &str, error: &str) {
        self.multi
            .println(format!("✗ Error on {}: {}", usb_path, error))
            .unwrap();
    }

    fn on_complete(&self) {
        self.main_pb.finish_with_message("done");
    }
}
