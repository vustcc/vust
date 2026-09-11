//! 构建脚本：编译期生成与打包所需的辅助逻辑。

use anyhow::{Context, Result};
use shadow_rs::ShadowBuilder;
use std::{fs, path::Path};

fn main() -> Result<()> {
    ShadowBuilder::builder()
        .deny_const(Default::default())
        .build()
        .unwrap();

    let dist_dir = Path::new("..").join("..").join("frontend").join("dist");
    println!("cargo:rerun-if-changed={}", dist_dir.display());
    if dist_dir.exists() {
        emit_rerun_if_changed(&dist_dir).with_context(|| {
            format!("Failed to track frontend assets at {}", dist_dir.display())
        })?;
    }

    Ok(())
}

fn emit_rerun_if_changed(path: &Path) -> Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            emit_rerun_if_changed(&entry.path())?;
        }
        return Ok(());
    }

    println!("cargo:rerun-if-changed={}", path.display());
    Ok(())
}
