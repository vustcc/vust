//! 操作审计写入端的领域枚举；公开 DTO 由 contracts 的 logging 模块提供。

/// 写入结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStatus {
    Success,
    Failed,
}

/// 写入影响级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformLogLevel {
    Info,
    Warning,
    Error,
}

/// 旧调用点迁移期间使用的领域提示，最终模块仍由事件注册表校正。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogModule {
    System,
    Auth,
    Docker,
    File,
    Process,
}
