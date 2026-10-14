# AgriTrace · 基于 Solana 的农产品溯源（加固版）

> **English TL;DR** — AgriTrace is an Anchor program on Solana storing **only the SHA-256 hash** of every
> farming / logistics event on-chain while raw data stays off-chain. Batches and stages are deterministic
> **PDAs** (`["batch", batch_id]`, `["stage", batch, index]`), so any client can derive and verify them
> without a lookup table. Access is gated by an on-chain **institution registry** with roles
> (Producer / Logistics / Retailer / Regulator), batch ids must carry the issuer's registered code
> (anti-squatting), and batches can be **transferred, recalled or sealed** — every mutation emits a
> structured event.

链上只放**哈希**，原始农事记录留在链下：省租金、不泄露商业数据，但任何人拿到原始记录
都能重算 SHA-256 与链上比对，验证「这条记录从写入起没被动过」。

---

## 1. 权限与账户模型

### 治理层

| 账户 | Seeds | 作用 |
|---|---|---|
| `Registry` | `["registry"]` | 单例：`admin`、`paused`（应急开关）、机构计数 |
| `Institution` | `["institution", 机构操作员地址]` | 机构代码 / 名称 / 角色 / 是否启用 / 建批计数 |
| `CodeClaim` | `["code", 机构代码]` | **代码唯一占用登记**，一个代码只能被一个机构注册 |

角色决定能力：

| 角色 | 建批次 | 追加环节 |
|---|---|---|
| `Producer` 生产方 | ✅ | ✅ |
| `Logistics` 仓储物流 | ❌ | ✅ |
| `Retailer` 零售终端 | ❌ | ✅ |
| `Regulator` 监管方 | ❌ | ❌（只读） |

### 业务层

| 账户 | Seeds | 作用 |
|---|---|---|
| `BatchRecord` | `["batch", batch_id]` | 一个批次一条，全局唯一 |
| `StageRecord` | `["stage", batch_record, index_le_u32]` | 一个流通环节一条，按序号排列 |

因为地址全是推导出来的，**扫码拿到批次号就能离线算出地址、读出整条链路**，不需要任何索引服务。

## 2. 指令

### 治理

| 指令 | 谁可以调 | 说明 |
|---|---|---|
| `initialize_registry` | 任何人（**部署后第一个调的**） | 初始化单例注册表，调用者成为 admin；重复调用会因账户已存在而失败 |
| `register_institution(code, name, role)` | admin + 该机构操作员共同签名 | 注册机构并占用代码；代码或地址重复都失败 |
| `set_institution_active(active)` | admin | 启用 / 停用机构。私钥泄露时的止血开关 |
| `set_paused(paused)` | admin | 全局应急暂停，暂停后所有写入指令拒绝 |
| `transfer_admin` | 当前 admin + 新 admin 共同签名 | 移交管理员，新 admin 必须签名，避免手滑填错地址把程序锁死 |

### 业务

| 指令 | 谁可以调 | 链上强制 |
|---|---|---|
| `create_batch(batch_id, region, stage, data_hash)` | 已启用 + `Producer` | 批次号必须以 `机构代码-` 开头、≤32 字节且全局唯一；产地 ≤24、环节 ≤24 字节；哈希 64 位十六进制 |
| `append_stage(stage, data_hash)` | 已启用 + 非 `Regulator` | 批次必须 `Active`；序号由链上 `stage_count` 决定；环节数 ≤64 |
| `transfer_batch()` | 当前 owner 机构 | 接收方必须是已启用的注册机构；批次必须 `Active` |
| `recall_batch(reason_hash)` | owner 机构 **或** admin | 批次必须 `Active`；召回原因哈希上链留痕 |
| `close_batch()` | 当前 owner 机构 | 批次必须 `Active` |

所有写入指令都会 `emit!` 结构化事件（`BatchCreated` / `StageAppended` / `BatchTransferred` /
`BatchRecalled` / `BatchClosed` / `InstitutionRegistered` / `InstitutionStatusChanged` /
`PausedChanged` / `AdminTransferred` / `RegistryInitialized`），索引器和前端直接订阅即可。

## 3. 链上不变量

1. **未注册、已停用的机构写不进任何数据**；`paused` 时全局拒绝写入；
2. **一个机构代码只能被一个机构占用**，批次号必须带本机构代码前缀 → 抢注别人批次号行不通；
3. 一个机构操作员地址只能注册一个机构（PDA 决定）；
4. 批次号全链唯一 —— 重复创建必然失败；
5. 环节序号从 0 开始严格递增，**不可能重复或跳号**；
6. 只有当前 owner（转让后就是新 owner）能封批 / 转让 / 召回；监管方只读；
7. **召回或封批之后，链上不再接受任何新环节**。

## 4. 错误码

| 错误 | 触发条件 |
|---|---|
| `EmptyField` | 字段为空或只有空白 |
| `InvalidCode` | 机构代码不是 2-12 位大写字母/数字 |
| `NameInvalid` | 机构名称超过 32 字节 |
| `BatchIdInvalid` | 批次号超过 32 字节 |
| `BatchIdPrefixMismatch` | 批次号没有带本机构代码前缀 |
| `RegionInvalid` / `StageInvalid` | 产地 / 环节名超过长度上限 |
| `InvalidHash` | 哈希不是 64 位十六进制 |
| `BatchNotActive` | 对已封批 / 已召回的批次写入 |
| `StageLimitReached` | 环节数达到 64 |
| `NotAdmin` | 非管理员调用治理指令 |
| `InstitutionInactive` | 机构未注册或已被停用 |
| `RoleNotAllowed` | 角色不允许该操作（例如监管方写入、物流方建批次） |
| `ProgramPaused` | 程序处于应急暂停 |
| `NotOwnerOrAdmin` | 非 owner 封批 / 转让，或非 owner 且非 admin 召回 |

长度一律按**字节**算（UTF-8）：一个汉字 3 字节，所以产地 8 个汉字正好到 24 字节上限。

## 5. 账户空间

`String` 序列化成 `4 字节长度前缀 + 内容`，Anchor 还会自动加 **8 字节 discriminator**：

```rust
Registry::LEN     = 32 + 1 + 8 + 1                                    =  42  → space = 50
CodeClaim::LEN    = 32 + 1                                            =  33  → space = 41
Institution::LEN  = 32 + (4+12) + (4+32) + 1 + 1 + 8 + 8 + 1          = 103  → space = 111
StageRecord::LEN  = 32 + 32 + (4+12) + 4 + (4+24) + (4+64) + 8 + 1    = 189  → space = 197
BatchRecord::LEN  = 32 + (4+12) + (4+32) + (4+24) + (4+24)
                    + (4+64) + (4+64) + 8 + 8 + 4 + 1 + 1             = 298  → space = 306
```

原版写的是 `8 + 32 + 64 + 32 + 64 + 8 = 208`：漏了 3 个长度前缀（12 字节），
也没有给 `region` 留位置 —— 字符串一长就会在写入时失败。这是本次重写修掉的核心 bug。

## 6. 安全边界（说清楚信任假设，评委一定会问）

**已经防住的：**

- 未注册 / 被停用的机构无法写入；监管角色无法写入；
- 抢注批次号、抢注机构代码（`CodeClaim`）；
- 跳号 / 重复的环节；批次号重复；
- 非 owner 封批、转让、召回；空字段、超长字段、伪造哈希格式；
- 应急停牌（`set_paused`）与单个机构止血（`set_institution_active`）。

**还需要注意的（诚实列出）：**

- **admin 是单把密钥**：生产环境建议改成多签（Squads）或绑定 program upgrade authority；
- `initialize_registry` 是"先到先得"：部署后要**立刻**调用，否则可能被别人抢先当上 admin；
- 追加环节对所有已启用机构开放（真实供应链需要多方写入），因此某个机构可以往别人的批次里
  写环节 —— 但每一笔都记录 `operator_code`，可追责；要更严可以加"机构白名单 + 批次参与方授权"；
- 机构代码停用后不释放（`CodeClaim` 仍占着），需要人工处理的历史遗留；
- 链下原始数据的真实性不在链上：链只能证明"这条记录没被改过"，不能证明"记录本身是真的"。

## 7. 在 Solana Playground 里怎么用

1. 替换三个文件：
   - `programs/agri-trace/src/lib.rs` ← [programs/agri-trace/src/lib.rs](programs/agri-trace/src/lib.rs)
   - `client/client.ts` ← [client/client.ts](client/client.ts)
   - `tests/agri_trace.ts` ← [tests/agri_trace.ts](tests/agri_trace.ts)
2. **Build → Deploy**（账户布局和 seeds 都变了，旧测试账户作废；program id 不变）。
3. **部署后第一件事**：跑一次 Client（它会自动调 `initialize_registry` 并把当前钱包设为 admin），
   或手动调 `initialize_registry`。
4. 点 **Test** 跑 13 个用例；点 Client 面板的 **Run** 跑完整演示流程。
5. 如果你的项目名不是 `agri_trace`，只改 `tests/agri_trace.ts` 里的
   `anchor.workspace.AgriTrace`。

### 兼容性注意

- 使用 `ctx.bumps.<account>`，需要 **anchor-lang ≥ 0.29**；0.28 及更早请换成
  `*ctx.bumps.get("<account>").unwrap()`。
- 故意没使用 `#[derive(InitSpace)]`（0.30 才引入），空间常量手写并逐项注明，0.29/0.30/0.31 都能编译。
- 哈希目前要求 **64 位十六进制**（SHA-256）；要换 base58/base64 只改 `validate_hash()`。
- 批次号上限 32 字节既是业务规则，也是 **Solana 单个 PDA 种子的硬上限**。

### 编译排坑：anchor 1.2.x 的 `require!` 是坏的

如果你（或别的项目）在 anchor-lang 1.2.x 上写 `require!(cond, MyError::X)`，会撞到这个：

```text
error[E0433]: failed to resolve: could not find `ErrorCode` in `$crate`
```

这不是用法错误。从 anchor-lang 1.2 起，错误类型被拆进独立的 `anchor-lang-error` crate
（crate 根只有 `pub use anchor_lang_error as error;`，`ErrorCode` 现在位于
`anchor_lang::error::ErrorCode`），而 `require!` 的展开里仍然写死旧的根路径
`$crate::ErrorCode` —— 所以**每一处** `require!` 都会失败，编译器只报第一个。

本项目用语义完全相同的 `ensure!` 规避（见 `lib.rs` 顶部）：

```rust
macro_rules! ensure {
    ($cond:expr, $error:path $(,)?) => {
        if !$cond {
            return ::core::result::Result::Err(error!($error));
        }
    };
}
```

`error!` 是 `#[error_code]` 为你生成的宏（展开成
`Error::from(AnchorError { error_name: 变体.name(), error_code_number: 变体.into(), .. })`），
所以第二个参数必须是**变体字面量**，不能传变量：`error!` 会对它求值三次
（`.name()` / `.into()` / `.to_string()`），传变量会因为所有权移动而编译失败。

## 8. 测试覆盖（13 个用例）

| 分组 | 用例 |
|---|---|
| 治理 | 注册表初始化 + 不可重复；非 admin 不能注册机构；机构字段/非法代码/代码占用/地址重复；停用后不可建批（恢复后可用）；程序暂停拒绝写入；移交管理员后旧 admin 失效 |
| 批次 | 批次号前缀校验；建批次字段与机构计数；哈希与长度校验（4 种）；环节序号递增与操作方留痕；跳号被拒；监管角色只读；转让后权限随之转移；召回后拒绝写入；封批后拒绝写入 |

测试里对第二个机构用**测试钱包转账**（`SystemProgram.transfer`）来充值，
不依赖 faucet，跑起来更稳。

## 9. 演示讲稿要点（配合项目视频）

1. **问题**：农产品溯源数据掌握在中心化平台手里，改一条记录没人知道；
2. **做法**：只把每条记录的 SHA-256 上链，原始数据留链下 —— 隐私和省钱兼得；
3. **可信**：批次和环节都是 PDA，扫码拿到批次号就能推导地址、读出完整链路；
4. **治理**：机构要在链上注册并被授予角色才能写，代码唯一、批次号带机构前缀；
5. **闭环**：环节序号链上自增不可跳号，可转让、可召回、可封批，全流程有事件可索引；
6. **验证**：任何人用原始记录重算哈希即可证明数据未被篡改。
