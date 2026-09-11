//! 特权 Linux 环境下的 loop-device 磁盘工具链验收。

use std::{fs::OpenOptions, path::PathBuf, process::Command};

struct LoopDevice {
    path: String,
    image: PathBuf,
}

impl Drop for LoopDevice {
    fn drop(&mut self) {
        let _ = Command::new("losetup")
            .args(["--detach", &self.path])
            .status();
        let _ = std::fs::remove_file(&self.image);
    }
}

fn command(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().unwrap();
    assert!(status.success(), "{program} failed");
}

#[test]
#[ignore = "requires root and Linux loop-device tools"]
fn creates_partition_then_formats_without_mounting() {
    assert_eq!(std::env::consts::OS, "linux");
    let image = std::env::temp_dir().join(format!("vust-disk-loop-{}.img", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&image)
        .unwrap();
    file.set_len(128 * 1024 * 1024).unwrap();
    let output = Command::new("losetup")
        .args(["--find", "--show", "--partscan"])
        .arg(&image)
        .output()
        .unwrap();
    assert!(output.status.success());
    let device = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let guard = LoopDevice {
        path: device.clone(),
        image,
    };
    command("parted", &["--script", &device, "mklabel", "gpt"]);
    command(
        "parted",
        &[
            "--script", &device, "mkpart", "primary", "ext4", "1MiB", "100%",
        ],
    );
    command("udevadm", &["settle", "--timeout=15"]);
    let partition = format!("{}p1", guard.path);
    let before_format = Command::new("blkid")
        .args(["-s", "TYPE", "-o", "value", &partition])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(before_format.stdout)
            .unwrap()
            .trim()
            .is_empty()
    );
    command("mkfs.ext4", &["-q", &partition]);
    let uuid = Command::new("blkid")
        .args(["-s", "UUID", "-o", "value", &partition])
        .output()
        .unwrap();
    assert!(uuid.status.success());
    assert!(!String::from_utf8(uuid.stdout).unwrap().trim().is_empty());
    let mounted = Command::new("findmnt")
        .args(["--noheadings", "--source", &partition])
        .output()
        .unwrap();
    assert!(
        !mounted.status.success(),
        "formatting must not mount the partition"
    );
}
