# fastgrep

**Agent 友好的快速正则搜索工具** — 基于 trigram 倒排索引，在大型代码仓库上实现 10-70x 加速。

## 问题背景

在 Agent 工作流中，`grep`/`rg` 是最高频的操作之一。当代码仓库规模达到数万文件时，ripgrep 的全文扫描往往需要数百毫秒甚至超过 15 秒。`fastgrep` 通过构建 trigram 倒排索引，先将候选文件缩小到极少数，再仅对这些文件执行完整正则匹配，从而大幅降低搜索延迟。

```
# 在 2244 个文件的仓库中搜索 "HashMap"
$ fastgrep search "HashMap"
[fastgrep] 16 matches in 6 candidates / 2244 total files (index: used)
#                          ↑ 仅扫描 6 个文件，而非全部 2244 个
```

## 快速开始

### 安装

```bash
# 一键安装（编译二进制 + 安装 Claude Code skill）
git clone https://github.com/user/fastgrep && cd fastgrep && bash install.sh

# 如果已经克隆了仓库
bash install.sh
```

这会：
1. 编译 `fastgrep` 和 `fastgrep-bench` 二进制 → `~/.local/bin/`
2. 安装 Claude Code skill → `~/.claude/skills/fastgrep/SKILL.md`

确保 `~/.local/bin` 在 `$PATH` 中。

### 30 秒上手

```bash
# 进入你的项目目录
cd /path/to/your/project

# 建索引（首次使用，后续搜索会自动维护）
fastgrep index

# 搜索！
fastgrep search "HashMap"
```

## 命令详解

### `fastgrep index` — 构建索引

扫描目录下所有文件，提取 trigram，写入 `.fastgrep/` 目录。

```bash
fastgrep index                    # 索引当前目录
fastgrep index --path /some/repo  # 索引指定目录
fastgrep index --incremental      # 增量重建（仅重新处理变更文件）
```

**输出示例：**
```
Building index for /data/home/user/linux...
Index built: 74521 files, 389204 trigrams in 2341ms
```

**增量重建**（`--incremental`）：
- 检测上次构建以来的变更/新增/删除文件（通过 mtime 或 git）
- 仅重新读取和提取变更文件的 trigram
- 从旧 posting list + 新 trigram 重建完整索引
- 变更超过 20% 时自动回退全量重建
- 大目录速度显著提升（如 75 万文件：全量 6 分钟 → 增量数秒）

**注意事项：**
- 自动尊重 `.gitignore`，跳过 `.git/` 和 `.fastgrep/` 目录
- 自动检测并跳过二进制文件（检查前 8KB 是否含 null 字节）
- 使用 Rayon 进行多核并行处理

### `fastgrep search` — 搜索

```bash
fastgrep search <PATTERN> [OPTIONS]
```

#### 基础搜索

```bash
# 字面量搜索
fastgrep search "HashMap"

# 正则搜索
fastgrep search "impl\s+\w+\s+for\s+\w+"

# Alternation
fastgrep search "(TODO|FIXME|HACK)"
```

#### 选项

| 选项 | 短写 | 说明 | 默认值 |
|------|------|------|--------|
| `--before-context <N>` | `-B` | 匹配行之前的上下文行数 | 0 |
| `--after-context <N>` | `-A` | 匹配行之后的上下文行数 | 0 |
| `--context <N>` | `-C` | 前后上下文行数（覆盖 -A/-B） | — |
| `--ignore-case` | `-i` | 大小写不敏感搜索 | false |
| `--type <EXT>` | `-t` | 按文件扩展名过滤 | — |
| `--glob <PATTERN>` | `-g` | 按 glob 模式过滤文件 | — |
| `--format <FMT>` | `-f` | 输出格式：`text` 或 `json` | text |
| `--path <DIR>` | `-p` | 搜索目录 | 当前目录 |
| `--auto-index` | — | 索引缺失或过期时自动重建 | true |

#### 实用示例

```bash
# 带上下文搜索 TODO/FIXME
fastgrep search "(TODO|FIXME)" -C 2

# 仅在 Rust 文件中搜索
fastgrep search "pub fn" -t rs

# 仅在 TSX 文件中搜索
fastgrep search "useState" -g "*.tsx"

# 大小写不敏感
fastgrep search "error" -i

# JSON 输出（适合 agent/脚本解析）
fastgrep search "HashMap" --format json

# 指定目录
fastgrep search "HashMap" --path /path/to/repo
```

#### 输出格式

**Text 格式**（默认，ripgrep 兼容）：
```
crates/fastgrep-core/src/index/builder.rs
42:use crate::ngram::extract::extract_trigrams;
```

文件名以洋红色高亮，行号以绿色高亮。尊重 `NO_COLOR` 环境变量。

**JSON 格式**（`--format json`，一行一条 JSON）：
```json
{"file":"src/index/builder.rs","line_number":42,"line":"use crate::ngram::extract::extract_trigrams;"}
```

**统计信息**（始终输出到 stderr）：
```
[fastgrep] 16 matches in 6 candidates / 2244 total files (index: used)
```

### `fastgrep status` — 查看索引状态

```bash
fastgrep status
```

**输出示例：**
```
Index status:
  Root:         /data/home/user/fastgrep
  Files:        2244
  Trigrams:     14827
  Commit:       a1b2c3d4e5f6...
  Fresh:        yes
  Index size:   416 KB (lookup: 231 KB, postings: 184 KB)
```

如果索引过期（HEAD commit 与构建时不同），会显示：
```
  Fresh:        NO — rebuild recommended
```

## 索引自动管理

默认行为（`--auto-index true`）：

1. **首次搜索**：无索引 → 自动构建
2. **后续搜索**：索引存在 → 检查 HEAD commit 是否匹配
3. **索引过期**：HEAD 已变化 → 增量重建（仅重新处理变更文件）
4. **索引新鲜**：直接使用，零额外开销
5. **Delta 积累过多**：变更文件超过 100 个 → 自动增量重建

**新鲜度模型：**
- **Git 仓库**：通过对比当前 HEAD commit 和索引中记录的 commit 判断。索引过期时触发增量重建（变更超 20% 自动回退全量）。索引新鲜但有未提交变更时，通过 delta 覆盖层搜索这些变更。
- **非 Git 目录**：索引记录构建时间戳。搜索时检测 mtime 比构建时间戳新的文件，通过 delta 覆盖层搜索。当累积的 delta 文件超过 100 个时，自动触发增量重建将变更合并进主索引。

```bash
# 不想自动建索引？手动管理：
fastgrep search "pattern" --auto-index false
```

## 何时使用 fastgrep vs rg

| 场景 | 推荐工具 |
|------|----------|
| 大仓库（>10k 文件），重复搜索 | **fastgrep** ✅ |
| 包含稀有字面量的模式 | **fastgrep** ✅ |
| Agent 工作流中的高频搜索 | **fastgrep** ✅ |
| 小仓库（<1k 文件） | rg（索引开销不值得） |
| 纯正则无字面量（`.*`、`\d+`） | rg（无法利用索引） |
| 一次性搜索 | rg（不需要建索引） |

**核心原则**：模式中字面量越多越稀有 → fastgrep 越有优势。

## 作为 Claude Code Skill 使用

运行 `bash install.sh` 时会自动安装 skill 到 `~/.claude/skills/fastgrep/SKILL.md`，之后在任何项目中都可以使用 `/fastgrep` 命令。

如果只想安装 skill（不编译二进制）：

```bash
mkdir -p ~/.claude/skills/fastgrep
cp .claude/skills/fastgrep/SKILL.md ~/.claude/skills/fastgrep/
```

安装后，Claude Code 在需要搜索大仓库时可以优先调用 `fastgrep search` 而非 `rg`。

## 压测：fastgrep vs ripgrep

### 快速开始

```bash
# 1. 准备语料（三选一）

# 方案 A：生成 10,000 文件的合成语料（推荐初次体验）
fastgrep-bench prepare --corpus medium --output ./testdata
CORPUS=./testdata/medium

# 方案 B：用自己的项目目录
CORPUS=/path/to/your/project

# 方案 C：克隆 Linux Kernel 极限压测（~74,000 文件）
git clone --depth 1 https://github.com/torvalds/linux.git ./testdata/linux-kernel
CORPUS=./testdata/linux-kernel

# 2. 跑压测（10 次迭代取中位数）
fastgrep-bench run --corpus $CORPUS --iterations 10 --output results.csv

# 3. 生成报告
fastgrep-bench report --input results.csv
```

> **rg 不在 PATH？** 用环境变量指定：`RG_PATH=/path/to/rg fastgrep-bench run ...`

### 测试模式

压测覆盖 9 种模式，代表典型的 Agent 搜索场景：

| 模式 | 类型 | 说明 |
|------|------|------|
| `fn` | 字面量（常见） | 几乎每个文件都有，索引无优势 |
| `HashMap` | 字面量（中等） | 部分文件有，索引可过滤大部分 |
| `SPDX-License-Identifier` | 字面量（稀有） | 极少文件有，**加速最明显** |
| `pub fn new` | 字面量（多词） | 多段 trigram 交集，过滤效果好 |
| `fn\s+\w+\s*\(` | 正则 | 含字面量 `fn`，可部分优化 |
| `use\s+\w+::\w+` | 正则 | 含字面量 `use`，可部分优化 |
| `impl\s+\w+\s+for\s+\w+` | 正则 | 含 `impl` + `for`，多段字面量 |
| `(TODO\|FIXME\|HACK)\b` | 正则（alternation） | 三选一，索引取并集 |
| `.*` | 不可优化 | 无字面量，回退全扫描（对照组） |

### 实测结果

#### Linux Kernel（92,790 文件，冷缓存 — Agent 真实场景）

```
| 模式                      | rg      | fastgrep | 加速比  |
|--------------------------|---------|----------|--------|
| KASAN_SHADOW_OFFSET      | 21.2s   | 0.52s    |  41x   |
| HashMap                  | 19.8s   | 0.30s    |  66x   |
```

#### Linux Kernel（92,790 文件，热缓存）

```
| 模式                      | rg (ms) | fastgrep (ms) | 加速比 |
|--------------------------|---------|---------------|--------|
| KASAN_SHADOW_OFFSET      |  158    |   188         |  0.8x  |
| HashMap                  |  163    |   182         |  0.9x  |
| EXPORT_SYMBOL（4 万匹配）  |  174    |   421         |  0.4x  |
```

**核心洞察**：fastgrep 的优势在于 **I/O 减少**——冷缓存（Agent 真实场景）下仅读索引 + 少量候选文件，实现 **41-66x** 加速。热缓存时 rg 的 SIMD 扫描难以超越。

> 详细压测方案见 [BENCHMARK.md](BENCHMARK.md)。

## 项目结构

```
fastgrep/
├── Cargo.toml                        # Workspace 根
├── crates/
│   ├── fastgrep-core/src/            # 核心库
│   │   ├── ngram/extract.rs          #   Trigram 提取 + FNV-1a 哈希
│   │   ├── ngram/weight.rs           #   CRC32 权重 + 字符对频率表
│   │   ├── index/format.rs           #   磁盘格式定义
│   │   ├── index/posting.rs          #   Varint 编码 + 集合运算
│   │   ├── index/builder.rs          #   并行索引构建
│   │   ├── index/writer.rs           #   索引序列化
│   │   ├── index/reader.rs           #   Mmap 读取 + 二分查找
│   │   ├── index/delta.rs            #   未提交变更覆盖层
│   │   ├── query/decompose.rs        #   正则 → trigram 分解
│   │   ├── query/plan.rs             #   查询计划优化
│   │   ├── query/execute.rs          #   搜索执行引擎
│   │   └── git.rs                    #   Git 集成
│   ├── fastgrep-cli/src/             # CLI
│   │   ├── main.rs                   #   clap 入口
│   │   ├── cmd/{index,search,status}.rs
│   │   └── output.rs                 #   输出格式化
│   └── fastgrep-bench/src/           # 压测工具
├── install.sh                        # 一键安装入口
├── scripts/install.sh                # 完整安装（编译 + skill）
└── .claude/skills/fastgrep/SKILL.md  # Claude Code Skill 定义
```

## 依赖

| 用途 | Crate |
|------|-------|
| 正则引擎 | `regex` + `regex-syntax` |
| 内存映射 | `memmap2` |
| 哈希 | `crc32fast`、FNV-1a（内置） |
| 字节序 | `byteorder` |
| CLI | `clap`（derive 模式） |
| 文件遍历 | `ignore`（.gitignore 感知） |
| Glob 匹配 | `globset` |
| 并行 | `rayon` |
| 错误处理 | `anyhow` + `thiserror` |
| Git | `gix` |
| 序列化 | `serde` + `serde_json` |

## License

MIT
