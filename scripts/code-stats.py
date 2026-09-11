#!/usr/bin/env python3
"""
VUST 代码行数统计脚本

该脚本用于统计 VUST 项目中的代码行数，支持按模块（后端、前端、运维脚本等）和语言类型进行分类统计。
支持对无后缀文件的 Shebang 智能识别，以及较健壮的多行注释过滤。
"""

import os
from collections import defaultdict
from pathlib import Path

# 项目根目录
ROOT = Path(__file__).resolve().parent.parent

# 排除的目录列表
EXCLUDE_DIRS = {
    ".git",
    "node_modules",
    "target",
    "dist",
    ".agents",
    ".vscode",
    ".vust",
    ".antigravitycli",
    ".venv",
    "venv",
    "__pycache__",
}

# 语言后缀名映射表
LANGUAGE_MAP = {
    ".rs": "Rust",
    ".ts": "TypeScript",
    ".tsx": "TSX",
    ".js": "JavaScript",
    ".mjs": "JavaScript",
    ".vue": "Vue",
    ".css": "CSS",
    ".scss": "SCSS",
    ".html": "HTML",
    ".py": "Python",
    ".sh": "Shell",
    ".toml": "TOML",
    ".yaml": "YAML",
    ".yml": "YAML",
}


class LanguageStats:
    """存储某类语言或模块的统计数据"""

    def __init__(self):
        self.files = 0
        self.total = 0
        self.code = 0
        self.comment = 0
        self.blank = 0


def detect_language(path: Path) -> str | None:
    """
    根据文件后缀或首行 Shebang 识别语言类型

    Args:
        path: 文件路径

    Returns:
        识别出的语言名称，若无法识别则返回 None
    """
    ext = path.suffix.lower()
    if ext in LANGUAGE_MAP:
        return LANGUAGE_MAP[ext]

    # 对于没有后缀或者后缀未知的文本文件，尝试读取首字节并解析 Shebang
    try:
        if not path.is_file():
            return None
        # 只读取前 128 字节，防止遇到大型二进制文件时读取整行导致性能或内存问题
        with path.open("rb") as f:
            header = f.read(128)
            if header.startswith(b"#!"):
                # 提取第一行并解码
                first_line = header.split(b"\n")[0].decode("utf-8", errors="ignore")
                if "bash" in first_line or "sh" in first_line or "zsh" in first_line:
                    return "Shell"
                elif "python" in first_line:
                    return "Python"
                elif "node" in first_line:
                    return "JavaScript"
    except Exception:
        pass
    return None


def get_module_name(path: Path) -> str:
    """
    根据文件相对项目根目录的路径，划分所属的子系统模块

    Args:
        path: 文件绝对路径

    Returns:
        模块名称 ('Backend' | 'Frontend' | 'Scripts & Ops' | 'Other')
    """
    try:
        rel_path = path.relative_to(ROOT)
        parts = rel_path.parts
        if not parts:
            return "Other"

        first_dir = parts[0]
        if first_dir == "crates":
            return "Backend"
        elif first_dir == "frontend":
            return "Frontend"
        elif first_dir in ("scripts", "deploy"):
            return "Scripts & Ops"
    except ValueError:
        pass
    return "Other"


def analyze_file(path: Path, lang: str):
    """
    分析单个文件，统计其总行数、代码行数、注释行数和空白行数

    Args:
        path: 文件路径
        lang: 语言类型

    Returns:
        (total, code, comment, blank) 元组
    """
    total = 0
    code = 0
    comment = 0
    blank = 0

    # 根据语言确定多行注释的起止标记
    block_start = None
    block_end = None
    if lang in {"Rust", "TypeScript", "TSX", "JavaScript", "CSS", "SCSS", "Vue"}:
        block_start = "/*"
        block_end = "*/"
    elif lang in {"HTML", "Vue"}:
        block_start = "<!--"
        block_end = "-->"

    # 单行注释的前缀
    single_prefixes = []
    if lang in {"Rust", "TypeScript", "TSX", "JavaScript", "Vue", "CSS", "SCSS"}:
        single_prefixes.append("//")
    if lang in {"Python", "Shell", "TOML", "YAML"}:
        single_prefixes.append("#")
    if lang in {"HTML", "Vue"}:
        single_prefixes.append("<!--")

    in_block_comment = False

    try:
        with path.open("r", encoding="utf-8", errors="ignore") as f:
            for line in f:
                total += 1
                stripped = line.strip()

                if not stripped:
                    blank += 1
                    continue

                # 处于多行注释块中
                if in_block_comment:
                    comment += 1
                    if block_end and block_end in stripped:
                        in_block_comment = False
                    continue

                # 检查是否是单行注释（或在一行内就结束的多行注释，如 /* comment */）
                is_single_comment = False
                for prefix in single_prefixes:
                    if stripped.startswith(prefix):
                        # 如果是 HTML 注释但在当前行结束，算作单行注释
                        if prefix == "<!--" and "-->" in stripped:
                            is_single_comment = True
                            break
                        # 如果是多行注释标记但在当前行结束
                        if prefix == "/*" and "*/" in stripped:
                            is_single_comment = True
                            break
                        is_single_comment = True
                        break

                if is_single_comment:
                    comment += 1
                    continue

                # 检查是否开启了多行注释
                if block_start and stripped.startswith(block_start):
                    comment += 1
                    if block_end and block_end not in stripped:
                        in_block_comment = True
                    continue

                # 忽略首行的 Shebang，但不作为空白行
                if total == 1 and stripped.startswith("#!"):
                    code += 1
                    continue

                code += 1
    except Exception:
        pass

    return total, code, comment, blank


def print_separator(char="=", width=80):
    """打印分隔线"""
    print(char * width)


def main():
    """主逻辑，执行项目扫描与统计输出"""
    total_files = 0
    total_lines = 0
    total_code = 0
    total_comment = 0
    total_blank = 0

    # 分语言和分模块的统计字典
    lang_stats = defaultdict(LanguageStats)
    module_stats = defaultdict(LanguageStats)

    for root, dirs, files in os.walk(ROOT):
        # 过滤黑名单目录
        dirs[:] = [d for d in dirs if d not in EXCLUDE_DIRS]

        for file in files:
            path = Path(root) / file

            # 识别文件语言
            lang = detect_language(path)
            if not lang:
                continue

            # 获取文件所属模块
            module = get_module_name(path)

            # 分析行数
            total, code, comment, blank = analyze_file(path, lang)

            # 语言统计更新
            l_stat = lang_stats[lang]
            l_stat.files += 1
            l_stat.total += total
            l_stat.code += code
            l_stat.comment += comment
            l_stat.blank += blank

            # 模块统计更新
            m_stat = module_stats[module]
            m_stat.files += 1
            m_stat.total += total
            m_stat.code += code
            m_stat.comment += comment
            m_stat.blank += blank

            # 全局统计更新
            total_files += 1
            total_lines += total
            total_code += code
            total_comment += comment
            total_blank += blank

    print()
    print_separator("=")
    print("VUST 项目代码行数统计 (Code Stats)")
    print(f"项目根目录: {ROOT}")
    print_separator("=")
    print()

    # 1. 整体概览
    print("【1. 整体概览 (Overall Overview)】")
    print(f"总文件数: {total_files}")
    print(f"总计行数: {total_lines}")

    code_pct = total_code / total_lines * 100 if total_lines else 0
    comment_pct = total_comment / total_lines * 100 if total_lines else 0
    blank_pct = total_blank / total_lines * 100 if total_lines else 0

    print(f"代码行数: {total_code:<10} ({code_pct:.1f}%)")
    print(f"注释行数: {total_comment:<10} ({comment_pct:.1f}%)")
    print(f"空白行数: {total_blank:<10} ({blank_pct:.1f}%)")
    print()

    # 2. 按模块统计
    print("【2. 按项目模块统计 (By Module)】")
    mod_header = (
        f"{'Module':<16}"
        f"{'Files':>8}"
        f"{'Total':>10}"
        f"{'Code':>10}"
        f"{'Comment':>10}"
        f"{'Blank':>10}"
        f"{'Code %':>10}"
    )
    print(mod_header)
    print_separator("-", 74)

    # 按照代码行数排序输出模块
    for mod, stats in sorted(
        module_stats.items(),
        key=lambda item: item[1].code,
        reverse=True,
    ):
        code_pct_in_mod = stats.code / stats.total * 100 if stats.total else 0
        print(
            f"{mod:<16}"
            f"{stats.files:>8}"
            f"{stats.total:>10}"
            f"{stats.code:>10}"
            f"{stats.comment:>10}"
            f"{stats.blank:>10}"
            f"{code_pct_in_mod:>9.1f}%"
        )
    print()

    # 3. 按语言统计
    print("【3. 按语言统计 (By Language)】")
    lang_header = (
        f"{'Language':<16}"
        f"{'Files':>8}"
        f"{'Total':>10}"
        f"{'Code':>10}"
        f"{'Comment':>10}"
        f"{'Blank':>10}"
        f"{'Code %':>10}"
    )
    print(lang_header)
    print_separator("-", 74)

    for lang, stats in sorted(
        lang_stats.items(),
        key=lambda item: item[1].code,
        reverse=True,
    ):
        code_pct_in_lang = stats.code / stats.total * 100 if stats.total else 0
        print(
            f"{lang:<16}"
            f"{stats.files:>8}"
            f"{stats.total:>10}"
            f"{stats.code:>10}"
            f"{stats.comment:>10}"
            f"{stats.blank:>10}"
            f"{code_pct_in_lang:>9.1f}%"
        )
    print()
    print_separator("=")


if __name__ == "__main__":
    main()
