//! 构建脚本：编译期生成所需的版本与编译元数据。

use anyhow::Result;
use shadow_rs::ShadowBuilder;

fn main() -> Result<()> {
    ShadowBuilder::builder()
        .deny_const(Default::default())
        .build()
        .unwrap();

    Ok(())
}
