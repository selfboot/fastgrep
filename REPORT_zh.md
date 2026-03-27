# fastgrep 技术实现报告

## 1. 系统架构总览

fastgrep 是一个基于 trigram 倒排索引的快速正则搜索工具。核心思想来源于信息检索领域：**先用倒排索引快速缩小候选集，再对候选文件执行精确匹配**。

### 1.1 架构图

```
┌─────────────────────────────────────────────────────┐
│                    CLI Layer                         │
│  fastgrep index | fastgrep search | fastgrep status  │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│                  Query Pipeline                      │
│                                                      │
│  ┌──────────┐   ┌──────────┐   ┌──────────────────┐ │
│  │ Decompose│──▶│   Plan   │──▶│     Execute      │ │
│  │ (regex →  │   │ (sort by │   │ (lookup →        │ │
│  │  trigrams)│   │  select.)│   │  intersect →     │ │
│  └──────────┘   └──────────┘   │  verify regex)   │ │
│                                 └──────────────────┘ │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│                  Index Layer                         │
│                                                      │
│  ┌───────────────┐  ┌─────────────┐  ┌───────────┐  │
│  │ index.lookup  │  │ index.post- │  │ index.meta│  │
│  │ (mmap, binary │  │ ings (varint│  │ (JSON,    │  │
│  │  search)      │  │  delta enc) │  │  file map)│  │
│  └───────────────┘  └─────────────┘  └───────────┘  │
└─────────────────────────────────────────────────────┘
```

### 1.2 完整搜索流程示例

以搜索 `impl.*Display` 为例：

```
输入: "impl.*Display"
                │
        ┌───────▼───────┐
  Step 1│  regex-syntax  │  解析为 HIR（High-level IR）
        │  AST 遍历      │
        └───────┬───────┘
                │  提取字面量子串: ["impl", "Display"]
        ┌───────▼───────┐
  Step 2│  Trigram 分解   │  "impl" → [imp, mpl]
        │                │  "Display" → [Dis, isp, spl, pla, lay]
        └───────┬───────┘
                │  must_match = [hash(imp), hash(mpl), hash(Dis), ...]
        ┌───────▼───────┐
  Step 3│  查询计划       │  按 posting list 大小排序
        │                │  最稀有的 trigram 排在前面
        └───────┬───────┘
                │  ordered = [hash(Dis), hash(isp), hash(mpl), ...]
        ┌───────▼───────┐
  Step 4│  索引查找       │  二分查找 lookup table
        │  + 交集运算     │  逐一 intersect posting lists
        │                │  早期终止：交集为空立即返回
        └───────┬───────┘
                │  candidate_file_ids = [12, 45, 203]
        ┌───────▼───────┐
  Step 5│  全文验证       │  仅对 3 个文件执行完整 regex
        │                │  （而非扫描全部 74k 文件）
        └───────┬───────┘
                │
             输出结果
```

---

## 2. 磁盘索引格式

索引存储在 `.fastgrep/` 目录下，由三个文件组成。

### 2.1 index.lookup — 查找表

```
偏移      大小      字段              说明
─────────────────────────────────────────────────
0x00      4B       magic             "FGLK" (0x46 0x47 0x4C 0x4B)
0x04      4B       version           1 (u32, little-endian)
─────────────────────────────────────────────────
0x08      16B      entry[0]          第一个查找条目
0x18      16B      entry[1]          第二个查找条目
...
```

每个查找条目（LookupEntry）固定 16 字节：

```
偏移      大小      字段              类型
─────────────────────────────────────────────────
+0x00     8B       ngram_hash        u64, little-endian, FNV-1a 哈希值
+0x08     4B       offset            u32, little-endian, postings 文件中的偏移
+0x0C     4B       len               u32, little-endian, posting list 字节长度
```

**关键设计决策**：

- 条目按 `ngram_hash` 升序排列，支持 O(log N) 二分查找
- 使用 mmap 映射到内存，无需加载整个文件
- 固定 16 字节条目大小使得随机访问成本为 O(1)

### 2.2 index.postings — 倒排列表

```
偏移      大小      字段              说明
─────────────────────────────────────────────────
0x00      4B       magic             "FGPS" (0x46 0x47 0x50 0x53)
0x04      4B       version           1 (u32, little-endian)
─────────────────────────────────────────────────
0x08      变长     posting_list[0]   第一个 trigram 的文件 ID 列表
...       变长     posting_list[N]   第 N 个 trigram 的文件 ID 列表
```

每个 posting list 的编码格式：

```
┌─────────┬────────┬────────┬────────┬─────┐
│ count   │ delta₀ │ delta₁ │ delta₂ │ ... │
│ (varint)│(varint)│(varint)│(varint)│     │
└─────────┴────────┴────────┴────────┴─────┘
```

- **count**: 列表中文件 ID 的数量
- **delta₀**: 第一个文件 ID（即与 0 的差值）
- **deltaᵢ**: 第 i 个文件 ID 与第 i-1 个的差值

**示例**：文件 ID 列表 `[5, 10, 20, 100, 1000]` 编码为：

```
varint(5)     → count = 5
varint(5)     → delta₀ = 5       → file_id = 0 + 5 = 5
varint(5)     → delta₁ = 5       → file_id = 5 + 5 = 10
varint(10)    → delta₂ = 10      → file_id = 10 + 10 = 20
varint(80)    → delta₃ = 80      → file_id = 20 + 80 = 100
varint(900)   → delta₄ = 900     → file_id = 100 + 900 = 1000
```

### 2.3 index.meta — 元数据

JSON 格式，包含：

```json
{
  "version": 1,
  "file_count": 2244,
  "trigram_count": 14827,
  "commit_hash": "a1b2c3d4e5f6...",
  "files": [
    "Cargo.toml",
    "crates/fastgrep-core/src/lib.rs",
    "..."
  ]
}
```

`files` 数组的索引位置即为文件 ID。搜索时通过文件 ID 反查路径。

### 2.4 磁盘空间占用分析

以 2244 文件、14827 个唯一 trigram 的仓库为例：

| 文件 | 计算 | 大小 |
|------|------|------|
| index.lookup | 8B header + 14827 × 16B | ~231 KB |
| index.postings | 8B header + Σ posting lists | ~184 KB |
| index.meta | JSON（含文件路径列表） | ~数十 KB |
| **合计** | | **~416 KB** |

---

## 3. Varint 编码

采用 LEB128（Little-Endian Base 128）变长整数编码，与 Protocol Buffers 使用的格式相同。

### 3.1 编码规则

每个字节的最高位（bit 7）为**继续标志**：
- `1` → 后面还有更多字节
- `0` → 这是最后一个字节

低 7 位承载实际数据，从低位到高位排列。

```
值            编码               字节数
───────────────────────────────────────
0             0x00               1
1             0x01               1
127           0x7F               1
128           0x80 0x01          2
300           0xAC 0x02          2
16384         0x80 0x80 0x01     3
u32::MAX      0xFF 0xFF 0xFF 0xFF 0x0F   5
```

### 3.2 编码过程示例

以编码 300 为例：

```
300 的二进制:  100101100
               ↓ 拆分为 7-bit 组
低 7 位:   0101100  (0x2C)
高 2 位:   10       (0x02)

第一个字节: 0x2C | 0x80 = 0xAC  (有续字节)
第二个字节: 0x02               (最后一个字节)

结果: [0xAC, 0x02]
```

### 3.3 Delta 编码的压缩效果

对于有序的文件 ID 序列，delta 编码将大数值转化为小差值，配合 varint 大幅缩减存储空间：

```
原始 ID:   [100, 105, 110, 200, 10000]
Delta:     [100,   5,   5,  90,  9800]

原始存储:  5 × 4B = 20B  (固定 u32)
Delta+Varint: ≈ 1 + 2 + 1 + 1 + 1 + 2 ≈ 8B  (60% 压缩)
```

---

## 4. Trigram 提取与哈希

### 4.1 FNV-1a 哈希算法

选择 FNV-1a 作为 trigram 哈希函数，原因：
- 计算极快（仅 XOR + 乘法）
- 对短输入（3 字节）分布均匀
- 确定性（相同输入始终产生相同哈希）

```rust
const FNV_OFFSET: u64 = 0xcbf29ce484222325;  // 64-bit offset basis
const FNV_PRIME:  u64 = 0x100000001b3;       // 64-bit prime

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= b as u64;                    // XOR
        hash = hash.wrapping_mul(FNV_PRIME);  // 乘以质数
    }
    hash
}
```

### 4.2 提取规则

从文件内容中提取 trigram 的规则：

1. **滑动窗口**：大小为 3 字节，步长为 1
2. **跳过换行**：任何包含 `\n`（0x0A）的 trigram 被丢弃
3. **去重**：每个文件内相同哈希值只记录一次
4. **最小长度**：内容不足 3 字节的文件不产生 trigram

```
输入: "Hello\nWorld"

窗口遍历:
  "Hel" → hash → ✓ 保留
  "ell" → hash → ✓ 保留
  "llo" → hash → ✓ 保留
  "lo\n" → ✗ 含换行，跳过
  "o\nW" → ✗ 含换行，跳过
  "\nWo" → ✗ 含换行，跳过
  "Wor" → hash → ✓ 保留
  "orl" → hash → ✓ 保留
  "rld" → hash → ✓ 保留

结果: 6 个唯一 trigram
```

### 4.3 为什么跳过换行 trigram

含换行的 trigram 几乎在所有文件中都出现（任何多行文件都有 `\n`），其 posting list 接近全集，对搜索无选择性价值。跳过它们可以：
- 减少索引大小
- 避免交集运算时的无效开销

---

## 5. 正则分解引擎

### 5.1 整体设计

使用 `regex-syntax` crate 将正则表达式解析为 HIR（High-level Intermediate Representation），然后递归遍历 HIR 树提取字面量子串，再从字面量中生成 trigram。

```
                正则表达式
                    │
          ┌─────────▼─────────┐
          │  regex-syntax 解析  │
          │  → HIR 树          │
          └─────────┬─────────┘
                    │
          ┌─────────▼─────────┐
          │  递归遍历 HIR       │
          │  提取 LiteralInfo  │
          └─────────┬─────────┘
                    │
          ┌─────────▼──────────┐
          │  字面量 → Trigram   │
          │  生成 must_match /  │
          │  alternatives      │
          └────────────────────┘
```

### 5.2 HIR 节点处理规则

| HIR 节点 | 处理方式 | 产出 |
|-----------|---------|------|
| `Literal("abc")` | 直接提取 | `Exact("abc")` |
| `Concat[a, b, c]` | 递归提取每部分的字面量 | `Conjunction([...])` |
| `Alternation[a \| b]` | 提取每个分支，任一分支无字面量则放弃 | `Alternation([...])` |
| `Capture(sub)` | 递归处理子模式 | 同子模式 |
| `Repetition{min≥1}` | 递归处理子模式 | 同子模式 |
| `Repetition{min=0}` | 不可优化 | `None` |
| `Class`（字符类） | 不可优化 | `None` |
| `Look`（断言） | 不可优化 | `None` |
| `Empty` | 不可优化 | `None` |

### 5.3 分解示例

**示例 1：纯字面量**
```
"HashMap"
  → HIR: Literal("HashMap")
  → Exact("HashMap")
  → must_match = trigrams("HashMap")
               = [hash("Has"), hash("ash"), hash("shM"), hash("hMa"), hash("Map")]
```

**示例 2：拼接含通配**
```
r"impl\s+Display"
  → HIR: Concat[Literal("impl"), Repetition{Class(\s), min=1}, Literal("Display")]
  → Conjunction(["impl", "Display"])
  → must_match = trigrams("impl") ∪ trigrams("Display")
               = [hash("imp"), hash("mpl"), hash("Dis"), hash("isp"),
                  hash("spl"), hash("pla"), hash("lay")]
```

**示例 3：Alternation**
```
r"(TODO|FIXME|HACK)"
  → HIR: Alternation[Literal("TODO"), Literal("FIXME"), Literal("HACK")]
  → alternatives = [
      trigrams("TODO"),    // [hash("TOD"), hash("ODO")]
      trigrams("FIXME"),   // [hash("FIX"), hash("IXM"), hash("XME")]
      trigrams("HACK"),    // [hash("HAC"), hash("ACK")]
    ]
```

**示例 4：不可优化**
```
r".*"
  → HIR: Repetition{Class(.), min=0}
  → None
  → optimizable = false → 回退全扫描
```

### 5.4 Alternation 的查询语义

对于 alternation `(A|B|C)`：

1. 每个分支独立计算 trigram 交集（conjunction）
2. 各分支结果取并集（union）
3. 与 must_match 的结果再取交集

```
candidates = (files_matching_A ∪ files_matching_B ∪ files_matching_C) ∩ files_matching_must
```

---

## 6. 查询计划优化

### 6.1 选择性排序

并非所有 trigram 的过滤效果相同。常见 trigram（如 `the`）的 posting list 可能包含大量文件，而稀有 trigram（如 `Dis`）的 posting list 很短。

查询计划器从索引中读取每个 trigram posting list 的字节长度（`len` 字段），按升序排列：

```
trigrams = [hash("Dis"), hash("imp"), hash("lay"), hash("mpl"), ...]
                 ↓              ↓            ↓           ↓
posting size:    42B           128B         256B         312B

排序后:
ordered = [hash("Dis"), hash("imp"), hash("lay"), hash("mpl")]
```

**优势**：最稀有的 trigram 先查，交集结果最小 → 后续交集运算数据量极小。

### 6.2 早期终止

在交集运算过程中，如果任何一步结果为空，立即返回：

```rust
for &hash in &plan.ordered_trigrams {
    let posting_list = reader.lookup(hash)?;
    // ↓ 如果某 trigram 不在索引中 → 不可能有匹配
    let posting_list = match reader.lookup(hash) {
        Some(list) => list,
        None => return Vec::new(),  // 早期终止
    };
    result = intersect(&current, &posting_list);
    if result.is_empty() {
        return Vec::new();  // 早期终止
    }
}
```

### 6.3 Posting List 交集算法

采用经典的**双指针归并交集**（merge-join），时间复杂度 O(n + m)：

```rust
fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Equal   => { result.push(a[i]); i += 1; j += 1; }
            Less    => i += 1,
            Greater => j += 1,
        }
    }
    result
}
```

前提：posting list 已按文件 ID 升序排列（构建时保证）。

---

## 7. 索引构建流程

### 7.1 完整管线

```
┌───────────────┐    ┌──────────────────┐    ┌──────────────────┐
│ 文件发现       │───▶│ 并行 Trigram 提取  │───▶│ 倒排索引构建      │
│ (ignore crate) │    │ (rayon par_iter)  │    │ (BTreeMap)       │
└───────────────┘    └──────────────────┘    └────────┬─────────┘
                                                      │
                     ┌──────────────────┐    ┌────────▼─────────┐
                     │ Git HEAD 检测    │───▶│ 写入磁盘          │
                     │ (gix crate)      │    │ (lookup+postings │
                     └──────────────────┘    │  +meta)          │
                                             └──────────────────┘
```

### 7.2 文件发现

使用 `ignore` crate（ripgrep 的底层遍历库），自动：
- 解析 `.gitignore`、`.gitignore_global`、`.git/info/exclude`
- 跳过隐藏文件
- 跳过 `.git/` 和 `.fastgrep/` 目录

返回按字典序排列的相对路径列表，数组索引即为文件 ID。

### 7.3 大文件跳过

为避免对大文件（如日志、数据文件、打包产物）进行不必要的读取和 trigram 提取，构建器在读取文件内容**之前**通过 `std::fs::metadata()` 检查文件大小：

```rust
const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

// Skip large files — check metadata BEFORE reading content
if let Ok(meta) = std::fs::metadata(&full_path) {
    if meta.len() > opts.max_file_size {
        skipped_large.fetch_add(1, Ordering::Relaxed);
        return None;
    }
}
```

**关键设计决策**：

- 默认阈值 `MAX_FILE_SIZE = 1 MB`，可通过 `BuildOptions.max_file_size` 配置
- 使用 `metadata()` 系统调用仅获取文件元信息，不读取文件内容，对大文件实现零 I/O 开销
- `BuildStats` 中新增 `skipped_large` 字段，报告跳过的大文件数量

### 7.4 实时进度输出

索引构建过程中通过 `AtomicUsize` 计数器提供实时进度反馈：

```rust
let processed = AtomicUsize::new(0);
let total = files.len();

// 在 par_iter 中：
let count = processed.fetch_add(1, Ordering::Relaxed) + 1;
if count % 500 == 0 || count == total {
    eprint!("\r  Extracting trigrams... {}/{}", count, total);
}
```

- 每处理 500 个文件输出一次进度（使用 `\r` 回车覆盖同一行）
- 最后一个文件处理完毕时也输出，确保显示 100% 完成
- 使用 `AtomicUsize` + `Ordering::Relaxed` 保证并行环境下的线程安全，同时最小化同步开销

### 7.5 文件 ID 重映射

并非所有被发现的文件都会被索引（二进制文件、大文件、空文件等会被跳过）。为避免 posting list 中出现大量未使用的稀疏 ID，构建器在提取 trigram 后进行 **ID 重映射**：

```rust
// 收集实际被索引的文件 ID
let mut indexed_file_ids: Vec<usize> = per_file_trigrams
    .iter().map(|(id, _)| *id).collect();
indexed_file_ids.sort_unstable();

// 建立 old_id → new_id 的映射（连续编号）
let mut id_remap: Vec<Option<u32>> = vec![None; files.len()];
let mut indexed_files: Vec<String> = Vec::with_capacity(indexed_file_ids.len());
for (new_id, &old_id) in indexed_file_ids.iter().enumerate() {
    id_remap[old_id] = Some(new_id as u32);
    indexed_files.push(files[old_id].clone());
}
```

**效果**：

- `index.meta` 的 `files` 数组只包含实际被索引的文件，不含被跳过的二进制/大文件
- posting list 中的文件 ID 连续紧凑（0, 1, 2, ...），delta 编码效率更高
- `file_count` 与 `indexed_count` 分离：前者是发现的总文件数，后者是实际索引的文件数

### 7.6 并行 Trigram 提取

```rust
let per_file_trigrams: Vec<(usize, HashSet<u64>)> = files
    .par_iter()           // Rayon 自动分配到所有 CPU 核心
    .enumerate()
    .filter_map(|(file_id, path)| {
        let data = fs::read(full_path).ok()?;

        // 二进制文件检测：前 8KB 含 null 字节则跳过
        if data[..8192.min(data.len())].contains(&0) {
            return None;
        }

        let trigrams = extract_trigrams_with_folded(&data);
        Some((file_id, trigrams))
    })
    .collect();
```

注意：当前使用 `extract_trigrams_with_folded()` 提取 trigram，同时存储原始大小写和 lowercase 归一化两套 trigram，以支持大小写不敏感搜索的索引加速（详见第 9 节）。

### 7.7 倒排索引构建

```rust
let mut trigram_map: BTreeMap<u64, Vec<u32>> = BTreeMap::new();

for (file_id, trigrams) in &per_file_trigrams {
    for &hash in trigrams {
        trigram_map.entry(hash).or_default().push(file_id as u32);
    }
}

// 确保每个 posting list 有序且无重复
for list in trigram_map.values_mut() {
    list.sort_unstable();
    list.dedup();
}
```

使用 `BTreeMap` 而非 `HashMap`，确保输出的 lookup table 天然有序（按 hash 值排列），无需额外排序。

### 7.8 二进制文件检测

采用简单但有效的启发式方法：

- 读取文件前 8192 字节
- 如果包含 `0x00`（null 字节），判定为二进制文件
- 二进制文件不参与索引

这种方法能正确识别图片、编译产物、字体等常见二进制格式，同时对文本文件无误判。

---

## 8. 内存映射读取

### 8.1 为什么使用 mmap

| 方案 | 内存占用 | 启动时间 | 随机访问 |
|------|---------|---------|---------|
| 全量加载到 Vec | O(文件大小) | 高（需读+解析） | O(1) |
| 按需 seek+read | O(1) | 低 | 高（系统调用开销） |
| **mmap** | **O(1)¹** | **低** | **O(1)** |

¹ 操作系统按需分页加载，实际物理内存占用远小于文件大小。

### 8.2 二分查找实现

```rust
pub fn lookup(&self, ngram_hash: u64) -> Option<Vec<u32>> {
    let data = &self.lookup_mmap[HEADER_SIZE..];
    let (mut lo, mut hi) = (0, self.entry_count);

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // 直接通过偏移量访问 mmap 内存
        let entry = self.read_lookup_entry(data, mid);

        match entry.ngram_hash.cmp(&ngram_hash) {
            Equal   => return Some(self.read_posting_list(entry.offset, entry.len)),
            Less    => lo = mid + 1,
            Greater => hi = mid,
        }
    }
    None
}
```

### 8.3 性能特征

- **查找复杂度**：O(log N)，N = trigram 数量
- **内存开销**：仅 mmap 描述符，OS 管理实际页面
- **冷启动**：首次访问触发页面加载，后续访问走 page cache
- **并发安全**：只读 mmap 天然线程安全

---

## 9. 大小写不敏感搜索

### 9.1 索引层：折叠 Trigram 存储

索引构建时使用 `extract_trigrams_with_folded()` 函数，对每个 3 字节滑动窗口**同时存储原始大小写和 lowercase 归一化两个版本**的 trigram 哈希：

```rust
pub fn extract_trigrams_with_folded(data: &[u8]) -> HashSet<u64> {
    let mut trigrams = HashSet::new();
    for window in data.windows(3) {
        if window.contains(&b'\n') { continue; }
        // 原始大小写
        trigrams.insert(fnv1a_hash(window));
        // Lowercase 归一化
        let folded: [u8; 3] = [
            window[0].to_ascii_lowercase(),
            window[1].to_ascii_lowercase(),
            window[2].to_ascii_lowercase(),
        ];
        trigrams.insert(fnv1a_hash(&folded));
    }
    trigrams
}
```

例如，文件内容包含 `"HashMap"` 时，索引中同时存储：
- 原始 trigram：`hash("Has")`, `hash("ash")`, `hash("shM")`, `hash("hMa")`, `hash("Map")`
- 折叠 trigram：`hash("has")`, `hash("ash")`, `hash("shm")`, `hash("hma")`, `hash("map")`

由于使用 `HashSet`，已经是小写的 trigram（如 `"ash"`）不会重复存储。

### 9.2 查询层：折叠 Trigram 提取

查询分解器 `decompose()` 接受 `case_insensitive: bool` 参数，根据标志选择不同的 trigram 提取函数：

```rust
pub fn decompose(pattern: &str, case_insensitive: bool) -> DecomposedQuery {
    let extract_fn = if case_insensitive {
        extract_literal_trigrams_folded  // 先转小写再提取
    } else {
        extract_literal_trigrams         // 原样提取
    };
    // ... 使用 extract_fn 提取 must_match 和 alternatives
}
```

`extract_literal_trigrams_folded()` 的实现非常简洁——先将字面量转为 ASCII 小写，再调用标准提取：

```rust
pub fn extract_literal_trigrams_folded(s: &str) -> Vec<u64> {
    let lower = s.to_ascii_lowercase();
    extract_literal_trigrams(&lower)
}
```

### 9.3 完整流程

```
搜索 "hashmap" (-i 模式):
  1. decompose("hashmap", case_insensitive=true)
     → extract_literal_trigrams_folded("hashmap")
     → trigrams of "hashmap": [hash("has"), hash("ash"), hash("shm"), hash("hma"), hash("map")]
  2. 在索引中查找这些折叠 trigram → 命中（因为索引已存储折叠版本）
  3. 获取候选文件集合（与大小写敏感搜索相同的索引加速路径）
  4. 对候选文件执行 (?i)hashmap 正则验证
```

**关键改进**：大小写不敏感搜索**不再回退全扫描**，而是通过折叠 trigram 走索引加速路径，与大小写敏感搜索享有相同的缩减比。

### 9.4 正则验证

无论是否使用索引加速，正则匹配阶段始终通过添加 `(?i)` 前缀实现大小写不敏感：

```rust
let regex_pattern = if opts.case_insensitive {
    format!("(?i){}", &opts.pattern)
} else {
    opts.pattern.clone()
};
```

---

## 10. Git 集成与索引新鲜度

### 10.1 新鲜度模型

```
索引构建时记录 HEAD commit hash → index.meta.commit_hash
搜索时比较 current HEAD vs stored commit

匹配   → 索引新鲜，直接使用
不匹配 → 索引过期，需要重建
```

#### 10.1.1 非 Git 目录处理

对于不在 Git 仓库中的目录，通过 `is_git_repo()` 检测：

```rust
pub fn is_git_repo(root: &Path) -> bool {
    gix::discover(root).is_ok()
}
```

当目录不是 Git 仓库时，`is_index_fresh()` 始终返回 `true`，信任已有索引。但通过 **基于 mtime 的 Delta 检测** 来捕获搜索间隔中的文件变更。

索引构建时，将 `SystemTime::now()` 记录为 `build_timestamp`（epoch 秒数）存入 `index.meta`。搜索时，`detect_fs_changes()` 遍历目录并：
- mtime > build_timestamp 的文件 → 视为新增/修改
- 索引中有但磁盘上不存在的文件 → 视为已删除

```rust
pub fn detect_fs_changes(
    root: &Path,
    indexed_files: &[String],
    build_timestamp: u64,
) -> Result<(Vec<String>, Vec<String>)> {
    let build_time = UNIX_EPOCH + Duration::from_secs(build_timestamp);

    // 遍历目录，stat 每个文件
    for entry in walker {
        if entry.metadata()?.modified()? > build_time {
            modified.push(rel_path);
        }
        current_files.insert(rel_path);
    }

    // 检测已删除文件
    let deleted = indexed_files.iter()
        .filter(|f| !current_files.contains(f.as_str()))
        .collect();

    Ok((modified, deleted))
}
```

这种方式非常轻量——`stat()` 比 `read()` 开销小得多。只有变更的文件才会被读取内容，通过现有的 `DeltaLayer` 进行搜索。

### 10.2 自动重建与 Delta 层集成

搜索命令（`search.rs`）的完整流程：

```rust
// 1. 检查索引新鲜度，必要时全量重建
if auto_index && !git::is_index_fresh(root, reader.commit_hash()) {
    eprintln!("Index is stale, rebuilding...");
    build_index(&opts)?;
    reader = IndexReader::open(root)?;
}

// 2. 构建 Delta 层（覆盖未提交变更）
let delta = build_delta_layer(root);

// 3. 执行搜索（传入 Delta 层）
execute_search(&reader, &search_opts, delta.as_ref())?;
```

`build_delta_layer()` 根据目录类型分支处理：
- **Git 仓库**：使用 `git status`/`git diff-index` 进行 delta 检测（行为不变）
- **非 Git 目录**：调用 `detect_fs_changes()`，传入索引的 `build_timestamp` 和文件列表，然后从结果构建 `DeltaLayer`

CLI 使用 `--no-auto-index` 标志（默认自动索引开启）来控制是否允许自动构建/刷新索引：

```
fastgrep search "pattern"              # 默认：自动索引 + delta
fastgrep search "pattern" --no-auto-index  # 跳过自动索引
```

### 10.3 变更检测

通过 `git status --porcelain` 和 `git diff-index` 检测变更：

```
git diff-index --name-status <stored_commit>
→ M	src/lib.rs          (modified)
→ A	src/new_file.rs     (added)
→ D	src/old_file.rs     (deleted)

git ls-files --others --exclude-standard
→ untracked_file.rs     (untracked)
```

### 10.4 Delta 层实现

`DeltaLayer` 为未提交变更提供覆盖层，**已完全集成到搜索管线中**：

```rust
pub struct DeltaLayer {
    // 新增/修改文件 → 重新提取的 trigram 集合
    pub modified_trigrams: BTreeMap<String, HashSet<u64>>,
    // 已删除文件的路径
    pub deleted_files: HashSet<String>,
}
```

`execute_search()` 接受 `Option<&DeltaLayer>` 参数，搜索流程：

1. **主索引查询**：正常查找 trigram 获取候选文件集
2. **排除已删除文件**：从候选中过滤掉 `delta.deleted_files` 中的文件
3. **搜索 Delta 文件**：遍历 `delta.modified_trigrams` 中的新增/修改文件，对未被主索引候选覆盖的文件执行额外搜索
4. **合并结果**：主索引结果与 Delta 搜索结果合并输出

```rust
// 排除已删除文件
let deleted_files: HashSet<&str> = match delta {
    Some(d) => d.deleted_files.iter().map(|s| s.as_str()).collect(),
    None => HashSet::new(),
};

// 主索引搜索时跳过已删除文件
for &file_id in &candidate_ids {
    let rel_path = reader.file_path(file_id)?;
    if deleted_files.contains(rel_path) { continue; }
    // ... 执行正则验证
}

// Delta 层：搜索新增/修改的文件
if let Some(delta) = delta {
    for path in delta.modified_trigrams.keys() {
        if searched_files.contains(path.as_str()) { continue; }
        // ... 对 delta 文件执行正则验证
    }
}
```

`SearchResult` 中包含 `delta_files` 字段，报告通过 Delta 层额外搜索的文件数量。

### 10.5 增量索引重建

对于大目录（如 QQMail 75 万文件，全量重建需 6 分钟），每次变更后全量重建不现实。增量重建机制避免重新读取未变更的文件。

#### 10.5.1 核心原理

增量重建 ≠ 原地修改磁盘文件（posting list 变长后 offset 全变，无法 patch）。实际做法：

```
加载旧索引 posting list → 重映射 file ID → 仅对变更文件重新提取 trigram → 合并 → 重写索引
```

核心优化：**跳过读取 99%+ 的文件内容**。仅读取变更/新增的文件。

#### 10.5.2 算法流程

```
incremental_rebuild(opts) → Result<Option<BuildStats>>:
  1. 打开旧 IndexReader，获取 build_timestamp
  2. 检测变更：
     - Git 仓库：changed_files_since(stored_commit)
     - 非 Git：detect_fs_changes(build_timestamp)
  3. 无变更 → 返回 None
  4. 变更比例 > 20% → 回退全量 build_index()
  5. discover_files() → 新文件列表，构建 path→new_id 映射
  6. 构建 old_id→new_id 映射（跳过 deleted，标记 modified）
  7. 遍历旧 lookup 表所有 entry：
     - 解码每个 posting list
     - 将旧 file_id 映射为新 file_id
     - 跳过 deleted/modified 文件（modified 会重新提取）
     - 构建新 trigram_map
  8. 仅对变更/新增文件并行提取 trigram（rayon）
  9. 合并新 trigram 到 trigram_map
  10. 重映射为连续 ID，write_index()
```

#### 10.5.3 触发机制

两种触发路径：

| 触发方式 | 时机 | 来源 |
|---------|------|------|
| **手动** | `fastgrep index --incremental` | 用户命令行调用 |
| **自动（搜索时）** | Delta 文件 ≥ 100 个 | `search.rs` 中 `build_delta_layer()` |
| **自动（索引过期）** | HEAD commit 不匹配 | `search.rs` 中 `run()` |

#### 10.5.4 安全机制

| 条件 | 行为 |
|------|------|
| 无变更 | 返回 `None`，跳过重建 |
| 变更比例 > 20% | 回退全量 `build_index()` |
| 旧索引无 `build_timestamp` | 回退全量 `build_index()` |
| 增量重建失败 | 回退全量重建或继续使用 delta 层 |

#### 10.5.5 性能

以 75 万文件目录（QQMail）为例：

| 场景 | 全量重建 | 增量重建 |
|------|---------|---------|
| 耗时 | ~6 分钟 | 数秒（与变更文件数成正比） |
| 读取文件 | 757,000 个 | 仅变更文件 |
| 瓶颈 | 读取所有文件内容 | 目录 stat 遍历 + 读取变更文件 |

---

## 11. 上下文行处理

### 11.1 算法

```rust
fn search_file(path, rel_path, regex, before_ctx, after_ctx) -> Vec<SearchMatch> {
    let lines: Vec<String> = read_all_lines(file);
    let mut context_lines_added: HashSet<usize> = HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            // 1. 添加 before-context
            for ctx_i in i.saturating_sub(before_ctx)..i {
                if context_lines_added.insert(ctx_i) { /* 添加 */ }
            }

            // 2. 添加匹配行自身
            if context_lines_added.insert(i) { /* 添加 */ }

            // 3. 添加 after-context
            for ctx_i in (i+1)..(i+after_ctx+1).min(lines.len()) {
                if context_lines_added.insert(ctx_i) { /* 添加 */ }
            }
        }
    }
}
```

### 11.2 去重机制

使用 `HashSet<usize>` 跟踪已添加的行号。当多个匹配行的上下文重叠时，确保每行只输出一次。

---

## 12. 文件过滤

### 12.1 文件类型过滤 (`-t`)

在候选文件上按扩展名过滤：

```rust
if let Some(ref ft) = file_type {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != ft.as_str() {
        return false;
    }
}
```

示例：`-t rs` 仅保留 `.rs` 文件。

### 12.2 Glob 过滤 (`-g`)

使用 `globset` crate 进行 glob 匹配：

```rust
let glob_matcher = globset::Glob::new(pattern)?.compile_matcher();
if !glob_matcher.is_match(path) {
    return false;
}
```

示例：`-g "*.tsx"` 仅保留 `.tsx` 文件。

### 12.3 过滤时机

过滤在索引查找**之后**、全文验证**之前**执行，减少不必要的文件 I/O：

```
索引查找 → 候选 ID 列表 → 类型/Glob 过滤 → 精简候选 → 全文 regex 验证
```

---

## 13. 权重系统（Sparse N-gram 预留）

### 13.1 字符对频率表

```rust
pub struct PairFrequencyTable {
    counts: Vec<u64>,   // 256 × 256 = 65536 项
    total: u64,
}
```

从语料中统计所有相邻字节对的出现频次。

### 13.2 选择性评分

```rust
pub fn ngram_selectivity(&self, bytes: &[u8]) -> f64 {
    // 取 n-gram 中所有字节对频率的最小值（瓶颈法）
    bytes.windows(2)
        .map(|w| self.frequency(w[0], w[1]))
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0)
}
```

最小频率对决定了整个 n-gram 的选择性：频率越低 → 越稀有 → 过滤效果越好。

### 13.3 CRC32 权重

```rust
pub fn crc32_weight(pair: &[u8; 2]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(pair);
    hasher.finalize()
}
```

用于 sparse n-gram 选择：对较长的字面量，不需要提取所有 trigram，而是选择 CRC32 权重最高（预估最稀有）的变长 n-gram 子集。此功能已预留接口，当前 MVP 使用固定 trigram。

---

## 14. 输出格式化

### 14.1 终端颜色检测

```rust
fn supports_color() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;        // 尊重 NO_COLOR 协议
    }
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}
```

- 尊重 [NO_COLOR](https://no-color.org/) 环境变量
- 使用 Rust 标准库 `IsTerminal` trait 检测 stdout 是否连接 TTY

### 14.2 ANSI 颜色方案

| 元素 | ANSI 序列 | 颜色 |
|------|-----------|------|
| 文件名 | `\x1b[35m...\x1b[0m` | 洋红 |
| 行号 | `\x1b[32m...\x1b[0m` | 绿色 |
| 匹配内容 | 无特殊着色 | 默认 |

与 ripgrep 的颜色方案保持一致。

---

## 15. 性能分析

### 15.1 理论复杂度

| 操作 | 时间复杂度 | 说明 |
|------|-----------|------|
| 索引构建 | O(F × L) | F = 文件数, L = 平均文件长度 |
| Trigram 查找 | O(log N) | N = 唯一 trigram 数 |
| Posting 解码 | O(P) | P = posting list 长度 |
| k 个 Trigram 交集 | O(k × P_min) | P_min = 最小 posting list 大小 |
| 全文验证 | O(C × L) | C = 候选文件数, L = 文件长度 |

### 15.2 实际测试数据

在 fastgrep 自身仓库（2244 文件）上的搜索：

| 模式 | 候选/总计 | 缩减比 |
|------|----------|--------|
| `"HashMap"` | 6 / 2244 | 374× |
| `"impl.*Display"` | 4 / 2244 | 561× |
| `".*"`（不可优化） | 2244 / 2244 | 1×（全扫描） |

### 15.3 Release 构建优化

```toml
[profile.release]
lto = true          # 跨 crate 链接时优化
codegen-units = 1   # 单编译单元，更激进优化
strip = true        # 剥离调试符号，缩小二进制
```

### 15.4 验证阶段性能优化

索引查找阶段（trigram → 候选文件 ID）本身已经很快（微秒级），实际瓶颈在**验证阶段**——对候选文件执行完整 regex 匹配。优化前验证阶段占总搜索时间的 70-90%。

#### 优化 1：mmap 替代 BufReader（零拷贝文件读取）

**优化前**：
```rust
let file = std::fs::File::open(path)?;
let reader = BufReader::new(file);
let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
// 问题：每行分配 String，物化整个文件，系统调用开销
```

**优化后**：
```rust
let mmap = unsafe { Mmap::map(&file)? };
let data = &mmap[..];  // 零拷贝：OS 按需分页加载
let line_starts = find_line_starts(data);
for line_idx in 0..line_count {
    if regex.is_match(line_bytes(line_idx)) { ... }  // 直接在 &[u8] 上匹配
}
```

#### 优化 2：bytes regex 替代 String regex

```rust
use regex::bytes::Regex as BytesRegex;  // 直接在 &[u8] 上匹配，省去 UTF-8 解码
```

#### 优化 3：rayon 并行文件验证

```rust
let matches: Vec<SearchMatch> = candidate_paths
    .par_iter()  // rayon 自动分配到所有 CPU 核心
    .flat_map(|(rel_path, full_path)| {
        search_file_mmap(full_path, rel_path, &regex, ...).unwrap_or_default()
    })
    .collect();
```

#### 优化 4：索引二分查找直接字节读取

```rust
#[inline]
fn read_lookup_entry(&self, data: &[u8], index: usize) -> LookupEntry {
    let bytes = &data[offset..offset + 16];
    let ngram_hash = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    // 消除 Cursor 分配 + trait 虚调用，纯指针算术
}
```

### 15.5 实测加速数据

#### Linux Kernel（92,790 索引文件，冷缓存）

冷缓存测试（每次运行前通过 `echo 3 > /proc/sys/vm/drop_caches` 清空 page cache）代表 Agent 首次搜索大仓库的真实场景：

| 模式 | rg | fastgrep | 加速比 |
|------|-----|----------|--------|
| `KASAN_SHADOW_OFFSET`（稀有，61 匹配） | 21.2s | 0.52s | **41x** |
| `HashMap`（稀有，15 匹配） | 19.8s | 0.30s | **66x** |

#### Linux Kernel（92,790 索引文件，热缓存）

所有文件在 page cache 中时，rg 扫描 9.2 万文件仅需 ~160ms。此时 fastgrep 的进程启动 + meta JSON 解析开销成为主导：

| 模式 | rg | fastgrep | 加速比 |
|------|-----|----------|--------|
| `KASAN_SHADOW_OFFSET`（稀有） | 158ms | 188ms | 0.8x |
| `HashMap`（稀有） | 163ms | 182ms | 0.9x |
| `EXPORT_SYMBOL`（常见，4 万匹配） | 174ms | 421ms | 0.4x |
| `impl\s+\w+\s+for\s+\w+`（正则） | 167ms | 865ms | 0.2x |

#### 加速规律

| 因素 | 影响 |
|------|------|
| **冷缓存**（真实场景） | **41-66x** — fastgrep 仅读索引 + 候选文件；rg 必须读全部 9.2 万文件 |
| **热缓存**（全在内存） | 0.2-0.9x — rg 的 SIMD 优化 mmap 扫描在 I/O 免费时难以超越 |
| 文件越多 | 冷缓存加速越大（索引消除更多 I/O） |
| 模式越稀有 | 加速越大（候选越少 → 验证 I/O 越少） |
| 不可优化模式（`.*`） | 无加速（回退全扫描） |

**核心洞察**：fastgrep 的主要优势在于 **I/O 减少**。通过仅读取索引文件 + 少量候选文件（而非全部 9.2 万文件），在磁盘/page-cache 成为瓶颈时实现极大加速。当所有文件已在 RAM 中（热缓存），rg 基于 SIMD 的原始扫描速度难以超越。

---

## 16. 测试策略

### 16.1 测试分层

| 层级 | 数量 | 覆盖范围 |
|------|------|---------|
| 单元测试 | 20 | 哈希确定性、varint 编解码、格式序列化、trigram 提取、查询分解、大小写不敏感分解 |
| 集成测试 | 12 | 端到端构建+搜索、正则、alternation、大小写、文件过滤、上下文行、全扫描回退、Delta 层新增文件、Delta 层删除文件排除、非 Git 目录 mtime Delta 检测、增量重建（增/改/删）、增量重建无变更 |
| 正确性测试 | 6 | 24 种模式逐行对比 naive grep、无漏匹配验证、大小写不敏感准确性、上下文行正确性、文件类型过滤正确性、边界情况 |
| **合计** | **38** | |

### 16.2 关键测试用例

**varint 边界值测试**：
```rust
for &val in &[0, 1, 127, 128, 300, 16384, u32::MAX] {
    assert_eq!(decode(encode(val)), val);
}
```

**索引缩减效果验证**：
```rust
assert!(result.candidate_count < result.total_files,
    "index should narrow candidates");
```

**全扫描回退验证**：
```rust
// r".*" 不可优化，必须全扫描
assert!(!result.used_index);
assert_eq!(result.candidate_count, result.total_files);
```

**Delta 层新增文件测试**：
```rust
// 索引构建后新增文件，不通过 delta 搜索不到
let result = execute_search(&reader, &search_opts, None).unwrap();
assert!(result.matches.is_empty());

// 通过 delta 层可以找到新文件
let delta = DeltaLayer::from_changed_files(root, &["new_feature.rs".to_string()], &[]).unwrap();
let result = execute_search(&reader, &search_opts, Some(&delta)).unwrap();
assert!(!result.matches.is_empty());
```

**Delta 层删除文件排除测试**：
```rust
// 删除文件后，通过 delta 层将其从结果中排除
let delta = DeltaLayer::from_changed_files(root, &[], &["notes.txt".to_string()]).unwrap();
let result = execute_search(&reader, &search_opts, Some(&delta)).unwrap();
assert!(!result.matches.iter().any(|m| m.file == "notes.txt"));
```

### 16.3 测试语料

集成测试使用 `tempfile` 在临时目录中创建 4 个测试文件（Rust、Python、文本），验证跨语言搜索的正确性。

---

## 17. 压测框架

### 17.1 测试矩阵

| 维度 | 取值 |
|------|------|
| 语料 | small（100 文件）、medium（10k 文件）、linux-kernel |
| 模式类型 | 字面量（常见/稀有/中等）、正则（函数声明/import/impl trait/TODO）、不可优化 |
| 迭代次数 | 可配置，默认 10 次取中位数 |

### 17.2 测试模式

```
literal_common:     "fn"
literal_rare:       "SPDX-License-Identifier"
literal_medium:     "HashMap"
regex_fn_decl:      r"fn\s+\w+\s*\("
regex_use_stmt:     r"use\s+\w+::\w+"
regex_impl_trait:   r"impl\s+\w+\s+for\s+\w+"
regex_todo:         r"(TODO|FIXME|HACK)\b"
regex_dot_star:     ".*"
```

### 17.3 输出格式

CSV 原始数据 + Markdown 报告表格：

```
| Pattern          | rg (ms) | fastgrep (ms) | Speedup | Matches |
|------------------|---------|---------------|---------|---------|
| literal_rare     | 138.0   | 2.0           | 69.0x   | 12      |
| regex_impl_trait | 487.0   | 8.0           | 60.9x   | 203     |
| regex_dot_star   | 156.0   | 165.0         | 0.9x    | 74000   |
```

---

## 18. 未来演进方向

### Phase 2: 性能优化
- [ ] Sparse n-gram：基于字符对频率选择变长 n-gram
- [ ] 完整 regex AST 遍历（当前仅处理 Literal/Concat/Alternation）
- [x] ~~索引加速的 case-insensitive 搜索（lowercase 归一化索引）~~ ✅ 已完成：构建时存储折叠 trigram，查询时使用折叠提取
- [x] ~~mmap + bytes regex 文件验证~~ ✅ 已完成：零拷贝文件读取，regex 直接在 `&[u8]` 上匹配
- [x] ~~并行文件验证~~ ✅ 已完成：Rayon `par_iter` 多核并行搜索候选文件
- [x] ~~索引二分查找直接字节读取~~ ✅ 已完成：消除 Cursor 分配开销

### Phase 3: 增量更新
- [x] ~~Delta 层实际集成到搜索管线~~ ✅ 已完成：`execute_search` 接受 `Option<&DeltaLayer>`，排除删除文件、搜索新增/修改文件
- [x] ~~非 Git 目录基于文件系统 mtime 的 Delta 检测~~ ✅ 已完成：在 `index.meta` 中记录 `build_timestamp`，搜索时遍历目录检测新增/修改/删除文件
- [x] ~~增量索引更新（仅处理变更文件，避免全量重建）~~ ✅ 已完成：`incremental_rebuild()` 加载旧 posting list、重映射 file ID、仅对变更文件重新提取 trigram、合并后重写索引。Delta 超阈值或索引过期时自动触发，也可通过 `fastgrep index --incremental` 手动触发。

### Phase 4: Agent 深度集成
- [ ] MCP Server 模式（常驻进程，避免重复 mmap 开销）
- [ ] 搜索结果排名（按相关性排序）
- [ ] 并行多模式搜索（一次查询多个 pattern）
