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
# 从源码构建并安装
cd /path/to/fastgrep
bash scripts/install.sh

# 或手动构建
cargo build --release -p fastgrep-cli
cp target/release/fastgrep ~/.local/bin/
```

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
```

**输出示例：**
```
Building index for /data/home/user/linux...
Index built: 74521 files, 389204 trigrams in 2341ms
```

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
3. **索引过期**：HEAD 已变化 → 自动重建
4. **索引新鲜**：直接使用，零额外开销

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

### 安装 Skill

```bash
cp skill/fastgrep.md ~/.claude/skills/
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

### 真实仓库测试结果（1,909 文件）

```
| Pattern          | rg (ms) | fastgrep (ms) | Speedup |
|------------------|---------|---------------|---------|
| literal_rare     |  175.8  |    49.8       |  3.5x   |
| literal_medium   |  173.9  |    47.1       |  3.7x   |
| literal_pub_fn   |  181.3  |    47.7       |  3.8x   |
| regex_impl_trait |  190.0  |    79.2       |  2.3x   |
| regex_use_stmt   |  180.8  |   114.6       |  1.5x   |
| regex_todo       |  251.7  |   180.3       |  1.4x   |
| literal_common   |  167.2  |   173.9       |  1.0x   |
| regex_fn_decl    |  176.3  |   177.3       |  1.0x   |
| regex_dot_star   | 4296.2  |  1162.9       |  3.7x   |
```

**规律**：模式越稀有加速越明显；仓库越大（文件越多）收益越高。

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
├── skill/fastgrep.md                 # Claude Code Skill 定义
└── scripts/install.sh                # 安装脚本
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
