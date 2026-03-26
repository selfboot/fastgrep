# fastgrep 压测方案

本文档提供完整的 fastgrep vs ripgrep 对比压测方案，方便在任何环境复现。

---

## 一、环境准备

### 1.1 安装 fastgrep + fastgrep-bench

```bash
git clone <repo-url> fastgrep
cd fastgrep
bash scripts/install.sh
```

安装完成后会得到两个二进制：

```
~/.local/bin/fastgrep       # 搜索工具
~/.local/bin/fastgrep-bench  # 压测工具
```

确认安装成功：

```bash
fastgrep --version
fastgrep-bench --help
```

> **注意**：如果提示 command not found，需要把 `~/.local/bin` 加入 PATH：
> ```bash
> export PATH="$HOME/.local/bin:$PATH"
> ```

### 1.2 安装 ripgrep

fastgrep-bench 需要 ripgrep 作为对照组。

**方式一：系统安装（推荐）**

```bash
# Ubuntu/Debian
sudo apt install ripgrep

# macOS
brew install ripgrep

# Cargo
cargo install ripgrep
```

**方式二：指定路径**

如果 `rg` 不在 PATH 中（比如 Claude Code 环境的 vendor 版本），通过环境变量指定：

```bash
export RG_PATH="/path/to/your/rg"
```

### 1.3 确认环境

```bash
# 验证两个工具都能找到
fastgrep --version
rg --version      # 或 $RG_PATH --version
```

---

## 二、准备测试语料

压测支持三种语料规模，选择一种或多种：

### 2.1 Small（100 文件）— 快速验证

```bash
fastgrep-bench prepare --corpus small --output ./testdata
```

生成 100 个 Rust 源文件到 `./testdata/small/`，每个约 115 行，包含 struct、impl、trait、TODO/FIXME 注释等典型代码结构。

### 2.2 Medium（10,000 文件）— 推荐基准

```bash
fastgrep-bench prepare --corpus medium --output ./testdata
```

生成 10,000 个 Rust 源文件到 `./testdata/medium/`，分布在 5 个子目录（src/、src/models/、src/handlers/、src/utils/、tests/），模拟真实项目结构。

### 2.3 Large（真实大仓库）— 极限压测

```bash
# Linux Kernel（~74,000 文件）
git clone --depth 1 https://github.com/torvalds/linux.git ./testdata/linux-kernel

# 或用你自己的大仓库
# 直接指定路径即可，无需拷贝
```

### 2.4 用自己的项目

不需要生成语料，直接用你的项目目录跑：

```bash
fastgrep-bench run --corpus /path/to/your/project --iterations 10 --output results.csv
```

---

## 三、运行压测

### 3.1 基本用法

```bash
fastgrep-bench run \
  --corpus ./testdata/medium \
  --iterations 10 \
  --output results.csv
```

**参数说明**：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--corpus <PATH>` | 必填 | 测试语料目录 |
| `--iterations <N>` | 10 | 每个模式重复次数，取中位数 |
| `--output <FILE>` | results.csv | 原始数据输出文件 |

### 3.2 指定工具路径

如果 `rg` 或 `fastgrep` 不在 PATH 中：

```bash
RG_PATH=/path/to/rg FG_PATH=/path/to/fastgrep \
  fastgrep-bench run --corpus ./testdata/medium --iterations 10 --output results.csv
```

### 3.3 运行过程

压测工具会自动执行以下步骤：

```
1. 打印配置信息（语料路径、迭代次数、工具路径）
2. 构建 fastgrep 索引（计时但不计入搜索耗时）
3. Warmup：各运行一次预热 OS page cache
4. 逐模式对比：
   对每个测试模式 × 每次迭代：
     a. 运行 rg，记录耗时和匹配行数
     b. 运行 fastgrep，记录耗时和匹配行数
5. 输出每个模式的中位数耗时、加速比、匹配数校验
6. 写入 CSV 原始数据
```

**输出示例**：

```
=== Benchmark Configuration ===
  Corpus:     ./testdata/medium
  Iterations: 10
  rg:         /usr/bin/rg
  fastgrep:   /home/user/.local/bin/fastgrep

Building fastgrep index...
Index built in 326ms

Warming up...

  literal_common      rg=  138.2ms  fg=   95.3ms    1.4x  matches: rg=20000 fg=20000 ✓
  literal_rare        rg=  135.7ms  fg=    2.1ms   64.6x  matches: rg=10000 fg=10000 ✓
  literal_medium      rg=  136.4ms  fg=    8.5ms   16.0x  matches: rg=1667 fg=1667 ✓
  ...

Results written to results.csv
```

- **✓** 表示 rg 和 fastgrep 匹配行数一致（结果正确）
- **✗ MISMATCH** 表示不一致（通常是因为 fastgrep 跳过了 >1MB 的大文件）

### 3.4 生成 Markdown 报告

```bash
fastgrep-bench report --input results.csv
```

输出标准 Markdown 表格，可以直接粘贴到文档里：

```markdown
# Benchmark Results

| Pattern | rg (ms) | fastgrep (ms) | Speedup | Matches |
|---------|---------|---------------|---------|---------|
| literal_rare | 135.7 | 2.1 | 64.6x | 10000 |
| ...
```

---

## 四、测试模式说明

压测覆盖 9 种模式，分为三类：

### 4.1 字面量模式（考察索引过滤效果）

| 名称 | 模式 | 选择性 | 说明 |
|------|------|--------|------|
| `literal_common` | `fn` | 低 | 几乎每个文件都有，索引无优势 |
| `literal_medium` | `HashMap` | 中 | 部分文件有，索引可过滤大部分 |
| `literal_rare` | `SPDX-License-Identifier` | 高 | 很少文件有，**索引加速最明显** |
| `literal_pub_fn` | `pub fn new` | 高 | 多词字面量，trigram 交集效果好 |

### 4.2 正则模式（考察 regex 分解能力）

| 名称 | 模式 | 说明 |
|------|------|------|
| `regex_fn_decl` | `fn\s+\w+\s*\(` | 含字面量 `fn` 和 `(`，可部分优化 |
| `regex_use_stmt` | `use\s+\w+::\w+` | 含字面量 `use`，可部分优化 |
| `regex_impl_trait` | `impl\s+\w+\s+for\s+\w+` | 含 `impl` 和 `for`，多段字面量 |
| `regex_todo` | `(TODO\|FIXME\|HACK)\b` | Alternation，三选一 |

### 4.3 边界情况

| 名称 | 模式 | 说明 |
|------|------|------|
| `regex_dot_star` | `.*` | **不可优化**，无字面量可提取，回退全扫描 |

---

## 五、如何解读结果

### 5.1 核心指标

| 指标 | 含义 |
|------|------|
| **Speedup** | rg 耗时 / fastgrep 耗时。>1x 表示 fastgrep 更快 |
| **Matches** | 匹配行数。两工具应一致，不一致需排查 |

### 5.2 预期结果规律

| 场景 | 预期加速 | 原因 |
|------|---------|------|
| 稀有字面量 + 大仓库 | **10-70x** | 索引将候选从数万缩小到个位数 |
| 中等字面量 + 大仓库 | **3-20x** | 候选缩小到几十~几百 |
| 常见字面量（`fn`） | **~1x** | 几乎所有文件都匹配，索引无法过滤 |
| 不可优化模式（`.*`） | **~1x 或更慢** | 回退全扫描，还有索引开销 |
| 小仓库（<1k 文件） | **<1x（更慢）** | 进程启动 + 索引加载开销超过收益 |

### 5.3 匹配数不一致的常见原因

| 情况 | 原因 | 是否正确 |
|------|------|---------|
| fastgrep < rg | fastgrep 跳过了 >1MB 的文件 | 正常 ✓ |
| fastgrep > rg | 不应该出现 | Bug ✗ |
| 两者都为 0 | 该模式在语料中没有匹配 | 正常 ✓ |

---

## 六、完整复现步骤（复制粘贴即可）

### 方案 A：用生成语料（推荐首次测试）

```bash
# 克隆并安装
git clone <repo-url> ~/fastgrep && cd ~/fastgrep
bash scripts/install.sh

# 生成 medium 语料（10,000 文件）
fastgrep-bench prepare --corpus medium --output ./testdata

# 跑压测（10 次迭代）
fastgrep-bench run --corpus ./testdata/medium --iterations 10 --output results.csv

# 生成报告
fastgrep-bench report --input results.csv
```

### 方案 B：用自己的项目

```bash
# 安装（同上）
cd ~/fastgrep && bash scripts/install.sh

# 直接对你的项目跑压测
fastgrep-bench run --corpus /path/to/your/project --iterations 10 --output results.csv
fastgrep-bench report --input results.csv
```

### 方案 C：Linux Kernel 极限测试

```bash
# 安装（同上）
cd ~/fastgrep && bash scripts/install.sh

# 克隆 Linux Kernel（~2GB，约 74,000 文件）
git clone --depth 1 https://github.com/torvalds/linux.git ./testdata/linux-kernel

# 跑压测（迭代次数可以少一些，每轮耗时较长）
fastgrep-bench run --corpus ./testdata/linux-kernel --iterations 5 --output results_linux.csv
fastgrep-bench report --input results_linux.csv
```

---

## 七、自定义测试模式

如果想测试自己的模式，目前需要修改源码：

编辑 `crates/fastgrep-bench/src/runner.rs` 中的 `PATTERNS` 数组：

```rust
const PATTERNS: &[(&str, &str)] = &[
    ("my_pattern", "your_search_term"),
    ("my_regex", r"your\s+regex\s+here"),
    // ... 保留或删除默认模式
];
```

然后重新编译：

```bash
cargo build --release -p fastgrep-bench
cp target/release/fastgrep-bench ~/.local/bin/
```

---

## 八、输出文件说明

### 8.1 results.csv（原始数据）

每行一次测量：

```csv
pattern_name,pattern,tool,iteration,wall_time_ms,match_count
literal_rare,SPDX-License-Identifier,rg,0,135.70,10000
literal_rare,SPDX-License-Identifier,fastgrep,0,2.10,10000
literal_rare,SPDX-License-Identifier,rg,1,134.20,10000
literal_rare,SPDX-License-Identifier,fastgrep,1,1.95,10000
...
```

可以用 Excel/Python/R 做进一步分析（画图、计算 P95 等）。

### 8.2 Markdown 报告（汇总）

`fastgrep-bench report` 输出到 stdout，按模式名排序，每个模式一行，取中位数。

---

## 九、注意事项

1. **首次运行较慢**：fastgrep 首次搜索需要建索引，后续搜索直接使用
2. **索引重建**：如果语料有变更（如 git commit），fastgrep 会自动重建索引，bench 的第一轮结果可能偏高
3. **公平对比**：压测工具会先 warmup 预热 OS page cache，确保 rg 和 fastgrep 在相同缓存条件下对比
4. **大文件跳过**：fastgrep 默认跳过 >1MB 的文件（模型、数据文件等），这会导致匹配数比 rg 少，属于正常设计取舍
5. **迭代次数**：建议 ≥5 次取中位数，避免单次波动。小语料 10 次，大语料 5 次即可
6. **CPU 负载**：压测期间避免其他 CPU 密集任务，影响结果稳定性
