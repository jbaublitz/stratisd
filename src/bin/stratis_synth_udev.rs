use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::Write,
};

use regex::Regex;

const DEVICEMAPPER_DIR: &str = "/dev/mapper";
const SYS_BLOCK_DIR: &str = "/sys/block";

fn get_stratis_dm_names() -> Result<Vec<String>, Box<dyn Error>> {
    let regex = Regex::new("/dev/mapper/stratis-1-[0-9a-f]{32}-thin-fs-[0-9a-f]{32}")?;
    let dir_handle = fs::read_dir(DEVICEMAPPER_DIR)?;
    let dm_devices = dir_handle
        .filter_map(|dirent| match dirent {
            Ok(d) => {
                let path = d.path();
                if !regex.is_match(&path.display().to_string()) {
                    return None;
                }
                let link_path = match fs::read_link(&path) {
                    Ok(lp) => lp,
                    Err(e) => {
                        eprintln!("Failed to read link for {}: {}", path.display(), e);
                        return None;
                    }
                };
                link_path
                    .file_name()
                    .and_then(|s| s.to_str().map(|s| s.to_string()))
            }
            Err(e) => {
                eprintln!("Failed to read entry from {}: {}", DEVICEMAPPER_DIR, e);
                None
            }
        })
        .collect();
    Ok(dm_devices)
}

fn send_udev_change_events(dm_names: Vec<String>) {
    for name in dm_names.into_iter() {
        let uevent_file = format!("{}/{}/uevent", SYS_BLOCK_DIR, name);
        let mut file = match OpenOptions::new().write(true).open(&uevent_file) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to open {}: {}", uevent_file, e);
                return;
            }
        };
        if let Err(e) = file.write_all(b"change") {
            eprintln!("Failed to write to {}: {}", uevent_file, e);
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let dm_names = get_stratis_dm_names()?;
    send_udev_change_events(dm_names);
    Ok(())
}
