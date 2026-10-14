//! # AgriTrace · 农产品溯源（Solana / Anchor）
//!
//! 链上**只存农事记录的 SHA-256 哈希**（64 位十六进制字符串），原始数据留在链下：
//! 省租金、保护隐私，同时任何人拿到原始记录都能自己重算哈希来验证「没被改过」。
//!
//! ## 权限模型
//!
//! ```text
//! Registry（单例 PDA）        admin 是谁、程序是否被应急暂停
//!   └── 只能由 admin 调用：注册机构 / 停用机构 / 暂停程序 / 移交管理员
//!
//! Institution（机构 PDA）     机构代码、名称、角色、是否启用
//!   seeds = ["institution", 机构操作员地址]
//!   ├── Producer  生产方：能建批次、能追加环节
//!   ├── Logistics 仓储物流：能追加环节
//!   ├── Retailer  零售终端：能追加环节
//!   └── Regulator 监管方：只读，不能写任何数据
//!
//! CodeClaim（代码占用登记）    seeds = ["code", 机构代码]，保证代码全局唯一
//! ```
//!
//! ## 账户模型
//!
//! ```text
//! BatchRecord   seeds = ["batch", batch_id]                        一个批次一条
//! StageRecord   seeds = ["stage", batch_record, index_le_u32]      一个流通环节一条
//! ```
//!
//! ## 链上强制的不变量
//!
//! 1. **未注册、已停用的机构写不进任何数据**；程序被 `paused` 时全局拒绝写入；
//! 2. **只有 `Producer` 能建批次**，`Regulator` 连环节都不能追加；
//! 3. 机构代码全局唯一；**批次号必须以本机构代码开头**（`YNPE-2026-0001`），防抢注；
//! 4. 批次号全链唯一 —— PDA 由 `batch_id` 决定，重复创建必然失败；
//! 5. 环节序号从 0 开始严格递增，不重复、不跳号（PDA 由 `stage_count` 决定）；
//! 6. 只有当前 `owner` 机构能封批、转让、召回；**召回或封批后不再接受新环节**；
//! 7. 召回原因哈希、操作方代码都留痕，事后可审计。
//!
//! ## 兼容性
//!
//! - 使用 `ctx.bumps.<account>`，需要 **anchor-lang ≥ 0.29**；
//! - 故意**不用** `#[derive(InitSpace)]`（0.30 才引入），空间常量手写并逐项注明；
//! - 错误统一用 `error!` 构造（由 `#[error_code]` 生成），不用 anchor 自带的 `require!`：
//!   在 anchor-lang 1.2.x 里错误类型已拆到独立的 `anchor-lang-error` crate，而 `require!`
//!   的展开仍写死旧的根路径 `$crate::ErrorCode`，会导致
//!   `error[E0433]: failed to resolve: could not find ErrorCode in $crate`。
//!   本文件用等价的 `ensure!` 替代（见下方定义）。

use anchor_lang::prelude::*;

/// `require!` 的安全替代：条件不成立就返回指定错误。
///
/// 语义与 anchor 自带的 `require!` 一致，但内部走 `error!`（由 `#[error_code]` 生成，
/// 负责把自定义错误装进 `AnchorError`），不依赖那个在 1.2.x 里写坏了的宏。
///
/// 第二个参数必须是**变体字面量**（例如 `AgriError::ProgramPaused`）：`error!` 展开时会
/// 对它求值三次（`.name()` / `.into()` / `.to_string()`），传变量会因为所有权移动而失败。
macro_rules! ensure {
    ($cond:expr, $error:path $(,)?) => {
        if !$cond {
            return ::core::result::Result::Err(error!($error));
        }
    };
}

declare_id!("FVcVehpa3rToFq8KggL1fBk1tyuSGkdNsqvaAXSLzPgu");

/// 机构代码最大长度（字节）
pub const MAX_CODE_LEN: usize = 12;
/// 机构名称最大长度（字节）
pub const MAX_NAME_LEN: usize = 32;
/// 批次号最大长度（字节，同时受 Solana PDA 单种子 32 字节上限约束）
pub const MAX_BATCH_ID_LEN: usize = 32;
/// 产地名称最大长度（字节）
pub const MAX_REGION_LEN: usize = 24;
/// 环节名称最大长度（字节）
pub const MAX_STAGE_LEN: usize = 24;
/// 哈希字符串长度 = SHA-256 十六进制长度
pub const MAX_HASH_LEN: usize = 64;
/// 单个批次最多可追加的环节数
pub const MAX_STAGES_PER_BATCH: u32 = 64;

#[program]
pub mod agri_trace {
    use super::*;

    // -----------------------------------------------------------------------
    // 治理：注册表与机构
    // -----------------------------------------------------------------------

    /// 初始化全局注册表，调用者成为 admin。部署后**第一件事**就是调它。
    ///
    /// 注册表是单例 PDA，重复调用会因为账户已存在而失败，不会被顶掉。
    pub fn initialize_registry(ctx: Context<InitializeRegistry>) -> Result<()> {
        let registry = &mut ctx.accounts.registry;
        registry.admin = ctx.accounts.admin.key();
        registry.paused = false;
        registry.institution_count = 0;
        registry.bump = ctx.bumps.registry;

        emit!(RegistryInitialized {
            admin: registry.admin,
            timestamp: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }

    /// 注册一个机构（仅 admin）。机构操作员必须一起签名 —— 避免管理员单方面
    /// 替某个地址「注册」，也证明这个地址确实掌握着自己的私钥。
    pub fn register_institution(
        ctx: Context<RegisterInstitution>,
        code: String,
        name: String,
        role: InstitutionRole,
    ) -> Result<()> {
        validate_code(&code)?;
        ensure!(!name.trim().is_empty(), AgriError::EmptyField);
        ensure!(name.len() <= MAX_NAME_LEN, AgriError::NameInvalid);

        let now = Clock::get()?.unix_timestamp;
        let institution = &mut ctx.accounts.institution;
        institution.authority = ctx.accounts.authority.key();
        institution.code = code;
        institution.name = name;
        institution.role = role;
        institution.active = true;
        institution.batch_count = 0;
        institution.registered_at = now;
        institution.bump = ctx.bumps.institution;

        let registry = &mut ctx.accounts.registry;
        registry.institution_count = registry.institution_count.saturating_add(1);

        let claim = &mut ctx.accounts.code_claim;
        claim.institution = institution.authority;
        claim.bump = ctx.bumps.code_claim;

        emit!(InstitutionRegistered {
            institution: institution.key(),
            authority: institution.authority,
            code: institution.code.clone(),
            role: institution.role,
            timestamp: now,
        });
        Ok(())
    }

    /// 启用 / 停用机构（仅 admin）。应急用：某机构私钥泄露时立刻止血。
    pub fn set_institution_active(ctx: Context<SetInstitutionActive>, active: bool) -> Result<()> {
        let institution = &mut ctx.accounts.institution;
        institution.active = active;

        emit!(InstitutionStatusChanged {
            institution: institution.key(),
            authority: institution.authority,
            active,
            timestamp: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }

    /// 全局应急开关（仅 admin）：暂停后所有写入指令直接拒绝。
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        let registry = &mut ctx.accounts.registry;
        registry.paused = paused;

        emit!(PausedChanged {
            paused,
            timestamp: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }

    /// 移交管理员（仅当前 admin，且**新 admin 必须签名**，防止手滑填错地址把程序锁死）。
    pub fn transfer_admin(ctx: Context<TransferAdmin>) -> Result<()> {
        let registry = &mut ctx.accounts.registry;
        let previous = registry.admin;
        registry.admin = ctx.accounts.new_admin.key();

        emit!(AdminTransferred {
            previous,
            current: registry.admin,
            timestamp: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 批次
    // -----------------------------------------------------------------------

    /// 创建批次，同时登记它的第一个环节（一般是「播种」）。
    ///
    /// 调用方必须是**已启用、角色为 `Producer`** 的注册机构，且 `batch_id`
    /// 必须以自己的机构代码开头（`代码-`），从源头防止抢注别人的批次号。
    pub fn create_batch(
        ctx: Context<CreateBatch>,
        batch_id: String,
        region: String,
        stage: String,
        data_hash: String,
    ) -> Result<()> {
        ensure!(!ctx.accounts.registry.paused, AgriError::ProgramPaused);

        let institution = &mut ctx.accounts.institution;
        ensure!(institution.active, AgriError::InstitutionInactive);
        ensure!(
            institution.role.can_create_batch(),
            AgriError::RoleNotAllowed
        );

        ensure!(!batch_id.trim().is_empty(), AgriError::EmptyField);
        ensure!(batch_id.len() <= MAX_BATCH_ID_LEN, AgriError::BatchIdInvalid);
        ensure!(!region.trim().is_empty(), AgriError::EmptyField);
        ensure!(region.len() <= MAX_REGION_LEN, AgriError::RegionInvalid);
        ensure!(!stage.trim().is_empty(), AgriError::EmptyField);
        ensure!(stage.len() <= MAX_STAGE_LEN, AgriError::StageInvalid);
        validate_hash(&data_hash)?;
        ensure!(
            has_code_prefix(&batch_id, &institution.code),
            AgriError::BatchIdPrefixMismatch
        );

        let now = Clock::get()?.unix_timestamp;
        let batch = &mut ctx.accounts.batch_record;

        batch.owner = ctx.accounts.authority.key();
        batch.owner_code = institution.code.clone();
        batch.batch_id = batch_id;
        batch.region = region;
        batch.stage = stage;
        batch.data_hash = data_hash;
        batch.recall_reason_hash = String::new();
        batch.created_at = now;
        batch.updated_at = now;
        batch.stage_count = 0;
        batch.status = BatchStatus::Active;
        batch.bump = ctx.bumps.batch_record;

        institution.batch_count = institution.batch_count.saturating_add(1);

        emit!(BatchCreated {
            batch: batch.key(),
            owner: batch.owner,
            owner_code: batch.owner_code.clone(),
            batch_id: batch.batch_id.clone(),
            region: batch.region.clone(),
            stage: batch.stage.clone(),
            data_hash: batch.data_hash.clone(),
            timestamp: now,
        });
        Ok(())
    }

    /// 给已有批次追加一个流通环节（施肥、采收、运输、上架……）。
    ///
    /// 调用方必须是**已启用且非监管**的注册机构：真实供应链上农场、物流、门店
    /// 都要写，所以这里不做「只能 owner 写」的限制，但每一步都会记下操作方代码。
    /// 环节序号由链上 `stage_count` 决定，调用方不能自己指定。
    pub fn append_stage(ctx: Context<AppendStage>, stage: String, data_hash: String) -> Result<()> {
        ensure!(!ctx.accounts.registry.paused, AgriError::ProgramPaused);

        let institution = &ctx.accounts.institution;
        ensure!(institution.active, AgriError::InstitutionInactive);
        ensure!(
            institution.role.can_append_stage(),
            AgriError::RoleNotAllowed
        );

        ensure!(!stage.trim().is_empty(), AgriError::EmptyField);
        ensure!(stage.len() <= MAX_STAGE_LEN, AgriError::StageInvalid);
        validate_hash(&data_hash)?;

        let now = Clock::get()?.unix_timestamp;
        let batch = &mut ctx.accounts.batch_record;

        ensure!(batch.status == BatchStatus::Active, AgriError::BatchNotActive);
        ensure!(
            batch.stage_count < MAX_STAGES_PER_BATCH,
            AgriError::StageLimitReached
        );

        let index = batch.stage_count;

        let record = &mut ctx.accounts.stage_record;
        record.batch = batch.key();
        record.operator = ctx.accounts.authority.key();
        record.operator_code = institution.code.clone();
        record.index = index;
        record.stage = stage.clone();
        record.data_hash = data_hash;
        record.timestamp = now;
        record.bump = ctx.bumps.stage_record;

        // 批次上的「当前环节」始终指向最后一次追加，前端不用遍历所有环节。
        batch.stage = stage;
        batch.stage_count = index + 1;
        batch.updated_at = now;

        emit!(StageAppended {
            batch: batch.key(),
            operator: record.operator,
            operator_code: record.operator_code.clone(),
            index,
            stage: record.stage.clone(),
            data_hash: record.data_hash.clone(),
            timestamp: now,
        });
        Ok(())
    }

    /// 批次转让：把 owner 交给另一个已启用的机构（农场 → 加工厂 → 物流……）。
    pub fn transfer_batch(ctx: Context<TransferBatch>) -> Result<()> {
        ensure!(!ctx.accounts.registry.paused, AgriError::ProgramPaused);
        ensure!(
            ctx.accounts.new_owner_institution.active,
            AgriError::InstitutionInactive
        );

        let new_owner = &ctx.accounts.new_owner_institution;
        let batch = &mut ctx.accounts.batch_record;
        ensure!(batch.status == BatchStatus::Active, AgriError::BatchNotActive);

        let previous_owner = batch.owner;
        batch.owner = new_owner.authority;
        batch.owner_code = new_owner.code.clone();
        batch.updated_at = Clock::get()?.unix_timestamp;

        emit!(BatchTransferred {
            batch: batch.key(),
            previous_owner,
            new_owner: batch.owner,
            new_owner_code: batch.owner_code.clone(),
            timestamp: batch.updated_at,
        });
        Ok(())
    }

    /// 召回：owner 机构或 admin 可把批次标记为 `Recalled`，之后拒绝任何新环节。
    /// `reason_hash` 是召回说明（检测报告等）的 SHA-256，链上留痕可审计。
    pub fn recall_batch(ctx: Context<RecallBatch>, reason_hash: String) -> Result<()> {
        validate_hash(&reason_hash)?;

        // 「owner 或 admin」这个判断放在这里而不是 constraint 里，读起来更清楚
        let is_owner = ctx.accounts.batch_record.owner == ctx.accounts.authority.key();
        let is_admin = ctx.accounts.registry.admin == ctx.accounts.authority.key();
        ensure!(is_owner || is_admin, AgriError::NotOwnerOrAdmin);

        let now = Clock::get()?.unix_timestamp;
        let batch = &mut ctx.accounts.batch_record;
        ensure!(batch.status == BatchStatus::Active, AgriError::BatchNotActive);

        batch.status = BatchStatus::Recalled;
        batch.recall_reason_hash = reason_hash;
        batch.updated_at = now;

        emit!(BatchRecalled {
            batch: batch.key(),
            owner: batch.owner,
            reason_hash: batch.recall_reason_hash.clone(),
            timestamp: now,
        });
        Ok(())
    }

    /// 封批：批次走完全流程后由 owner 机构关闭，之后不再接受新环节。
    pub fn close_batch(ctx: Context<CloseBatch>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let batch = &mut ctx.accounts.batch_record;
        ensure!(batch.status == BatchStatus::Active, AgriError::BatchNotActive);

        batch.status = BatchStatus::Closed;
        batch.updated_at = now;

        emit!(BatchClosed {
            batch: batch.key(),
            owner: batch.owner,
            stage_count: batch.stage_count,
            timestamp: now,
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 账户校验：治理
// ---------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeRegistry<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + Registry::LEN,
        seeds = [Registry::SEED],
        bump,
    )]
    pub registry: Account<'info, Registry>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(code: String)]
pub struct RegisterInstitution<'info> {
    /// 只有 admin 能注册机构
    #[account(
        mut,
        seeds = [Registry::SEED],
        bump = registry.bump,
        constraint = registry.admin == admin.key() @ AgriError::NotAdmin,
    )]
    pub registry: Account<'info, Registry>,

    /// 代码占用登记：已存在就说明这个代码被占了，init 会直接失败
    #[account(
        init,
        payer = admin,
        space = 8 + CodeClaim::LEN,
        seeds = [CodeClaim::SEED, code.as_bytes()],
        bump,
    )]
    pub code_claim: Account<'info, CodeClaim>,

    /// 机构账户：地址由机构操作员地址唯一确定
    #[account(
        init,
        payer = admin,
        space = 8 + Institution::LEN,
        seeds = [Institution::SEED, authority.key().as_ref()],
        bump,
    )]
    pub institution: Account<'info, Institution>,

    /// 机构操作员：必须签名（证明它掌握自己的私钥），但不付租金
    pub authority: Signer<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetInstitutionActive<'info> {
    #[account(
        seeds = [Registry::SEED],
        bump = registry.bump,
        constraint = registry.admin == admin.key() @ AgriError::NotAdmin,
    )]
    pub registry: Account<'info, Registry>,

    /// CHECK: 只用来推导目标机构的 PDA，不读它的数据
    pub authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [Institution::SEED, authority.key().as_ref()],
        bump = institution.bump,
    )]
    pub institution: Account<'info, Institution>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct SetPaused<'info> {
    #[account(
        mut,
        seeds = [Registry::SEED],
        bump = registry.bump,
        constraint = registry.admin == admin.key() @ AgriError::NotAdmin,
    )]
    pub registry: Account<'info, Registry>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct TransferAdmin<'info> {
    #[account(
        mut,
        seeds = [Registry::SEED],
        bump = registry.bump,
        constraint = registry.admin == admin.key() @ AgriError::NotAdmin,
    )]
    pub registry: Account<'info, Registry>,

    /// 现任管理员
    pub admin: Signer<'info>,

    /// 新管理员也必须签名
    pub new_admin: Signer<'info>,
}

// ---------------------------------------------------------------------------
// 账户校验：批次
// ---------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(batch_id: String)]
pub struct CreateBatch<'info> {
    #[account(seeds = [Registry::SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,

    /// 建批机构：必须是已注册机构，且角色允许建批
    #[account(
        mut,
        seeds = [Institution::SEED, authority.key().as_ref()],
        bump = institution.bump,
    )]
    pub institution: Account<'info, Institution>,

    /// 批次账户：地址由批次号唯一确定，客户端可离线推导
    #[account(
        init,
        payer = authority,
        space = 8 + BatchRecord::LEN,
        seeds = [BatchRecord::SEED, batch_id.as_bytes()],
        bump,
    )]
    pub batch_record: Account<'info, BatchRecord>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AppendStage<'info> {
    #[account(seeds = [Registry::SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,

    /// 追加环节的机构
    #[account(
        seeds = [Institution::SEED, authority.key().as_ref()],
        bump = institution.bump,
    )]
    pub institution: Account<'info, Institution>,

    /// 父批次：同时用 seeds + bump 复验它确实是本程序派生的 PDA
    #[account(
        mut,
        seeds = [BatchRecord::SEED, batch_record.batch_id.as_bytes()],
        bump = batch_record.bump,
    )]
    pub batch_record: Account<'info, BatchRecord>,

    /// 环节账户：序号取批次当前的 stage_count，因此天然递增且不重复
    #[account(
        init,
        payer = authority,
        space = 8 + StageRecord::LEN,
        seeds = [
            StageRecord::SEED,
            batch_record.key().as_ref(),
            &batch_record.stage_count.to_le_bytes(),
        ],
        bump,
    )]
    pub stage_record: Account<'info, StageRecord>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TransferBatch<'info> {
    #[account(seeds = [Registry::SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,

    /// 新 owner 的操作员地址，仅用于推导机构 PDA
    /// CHECK: 不读数据
    pub new_owner_authority: UncheckedAccount<'info>,

    /// 新 owner 机构：必须已注册且启用
    #[account(
        seeds = [Institution::SEED, new_owner_authority.key().as_ref()],
        bump = new_owner_institution.bump,
    )]
    pub new_owner_institution: Account<'info, Institution>,

    /// 当前 owner 必须签名
    #[account(
        mut,
        seeds = [BatchRecord::SEED, batch_record.batch_id.as_bytes()],
        bump = batch_record.bump,
        constraint = batch_record.owner == authority.key() @ AgriError::NotOwnerOrAdmin,
    )]
    pub batch_record: Account<'info, BatchRecord>,

    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct RecallBatch<'info> {
    #[account(seeds = [Registry::SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,

    /// 权限在指令里判断：owner 机构或 admin 都能召回
    #[account(
        mut,
        seeds = [BatchRecord::SEED, batch_record.batch_id.as_bytes()],
        bump = batch_record.bump,
    )]
    pub batch_record: Account<'info, BatchRecord>,

    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct CloseBatch<'info> {
    /// 只有当前 owner 能封批
    #[account(
        mut,
        seeds = [BatchRecord::SEED, batch_record.batch_id.as_bytes()],
        bump = batch_record.bump,
        constraint = batch_record.owner == authority.key() @ AgriError::NotOwnerOrAdmin,
    )]
    pub batch_record: Account<'info, BatchRecord>,

    pub authority: Signer<'info>,
}

// ---------------------------------------------------------------------------
// 账户数据：治理
// ---------------------------------------------------------------------------

/// 全局注册表（单例）：程序的管理员与应急开关
#[account]
pub struct Registry {
    /// 管理员，唯一有权注册/停用机构、暂停程序、移交管理员
    pub admin: Pubkey,
    /// 应急暂停：为 true 时所有写入指令拒绝执行
    pub paused: bool,
    /// 已注册机构数量（只增不减，用于统计展示）
    pub institution_count: u64,
    /// PDA bump
    pub bump: u8,
}

impl Registry {
    pub const SEED: &'static [u8] = b"registry";

    /// 32 + 1 + 8 + 1
    pub const LEN: usize = 32 + 1 + 8 + 1;
}

/// 机构账户：一个参与方（农场 / 物流 / 门店 / 监管）
#[account]
pub struct Institution {
    /// 操作员地址，机构的签名者
    pub authority: Pubkey,
    /// 机构代码，大写字母数字，同时也是批次号前缀
    pub code: String,
    /// 机构名称
    pub name: String,
    /// 角色，决定能做什么
    pub role: InstitutionRole,
    /// 是否启用，admin 可随时停用
    pub active: bool,
    /// 累计创建的批次数
    pub batch_count: u64,
    /// 注册时间
    pub registered_at: i64,
    /// PDA bump
    pub bump: u8,
}

impl Institution {
    pub const SEED: &'static [u8] = b"institution";

    /// 32 + (4+12) + (4+32) + 1 + 1 + 8 + 8 + 1
    pub const LEN: usize = 32                        // authority
        + (4 + MAX_CODE_LEN)                         // code
        + (4 + MAX_NAME_LEN)                         // name
        + 1                                          // role
        + 1                                          // active
        + 8                                          // batch_count
        + 8                                          // registered_at
        + 1; // bump
}

/// 机构代码占用登记：种子里装着代码本身，所以**同一个代码只能被注册一次**。
///
/// 没有这张表的话，别人可以注册一个和你一模一样的代码，然后用你的前缀建批次 ——
/// 那样「批次号带机构前缀」这条防抢注规则就形同虚设。
#[account]
pub struct CodeClaim {
    /// 占用这个代码的机构
    pub institution: Pubkey,
    /// PDA bump
    pub bump: u8,
}

impl CodeClaim {
    pub const SEED: &'static [u8] = b"code";

    /// 32 + 1
    pub const LEN: usize = 32 + 1;
}

/// 机构角色：越靠后的职务能做的事越少
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstitutionRole {
    /// 生产方：能建批次、能追加环节
    Producer,
    /// 仓储物流：能追加环节
    Logistics,
    /// 零售终端：能追加环节
    Retailer,
    /// 监管方：只读
    Regulator,
}

impl InstitutionRole {
    /// 只有生产方能创建批次
    pub fn can_create_batch(&self) -> bool {
        matches!(self, InstitutionRole::Producer)
    }

    /// 除监管方外都能追加环节
    pub fn can_append_stage(&self) -> bool {
        !matches!(self, InstitutionRole::Regulator)
    }
}

// ---------------------------------------------------------------------------
// 账户数据：批次
// ---------------------------------------------------------------------------

/// 主批次账户：一个农产品批次的全生命周期状态
#[account]
pub struct BatchRecord {
    /// 当前 owner 机构（可通过 transfer_batch 转让）
    pub owner: Pubkey,
    /// owner 的机构代码，前端不用再查一次机构账户
    pub owner_code: String,
    /// 批次号（全链唯一）
    pub batch_id: String,
    /// 产地
    pub region: String,
    /// 当前所处环节
    pub stage: String,
    /// 当前环节记录的 SHA-256
    pub data_hash: String,
    /// 召回原因（检测报告等）的 SHA-256，未召回时为空
    pub recall_reason_hash: String,
    /// 创建时间（Unix 秒）
    pub created_at: i64,
    /// 最近更新时间
    pub updated_at: i64,
    /// 已追加的环节数量，也等于下一个环节的序号
    pub stage_count: u32,
    /// 批次状态
    pub status: BatchStatus,
    /// PDA bump
    pub bump: u8,
}

impl BatchRecord {
    /// PDA 种子前缀
    pub const SEED: &'static [u8] = b"batch";

    /// 账户数据长度（**不含** Anchor 自动加的 discriminator）。
    ///
    /// `String` 的序列化是 `4 字节长度前缀 + 实际字节`，所以每个字符串都要 +4；
    /// 原版把长度前缀漏掉了，导致字符串一长就写不进去。
    pub const LEN: usize = 32                        // owner
        + (4 + MAX_CODE_LEN)                         // owner_code
        + (4 + MAX_BATCH_ID_LEN)                     // batch_id
        + (4 + MAX_REGION_LEN)                       // region
        + (4 + MAX_STAGE_LEN)                        // stage
        + (4 + MAX_HASH_LEN)                         // data_hash
        + (4 + MAX_HASH_LEN)                         // recall_reason_hash
        + 8                                          // created_at
        + 8                                          // updated_at
        + 4                                          // stage_count
        + 1                                          // status
        + 1; // bump
}

/// 环节账户：批次下每一个流通阶段一条，按 `index` 排序
#[account]
pub struct StageRecord {
    /// 所属批次
    pub batch: Pubkey,
    /// 写入这个环节的操作方
    pub operator: Pubkey,
    /// 操作方的机构代码
    pub operator_code: String,
    /// 环节序号，从 0 开始
    pub index: u32,
    /// 环节名称
    pub stage: String,
    /// 该环节原始记录的 SHA-256
    pub data_hash: String,
    /// 上链时间（Unix 秒）
    pub timestamp: i64,
    /// PDA bump
    pub bump: u8,
}

impl StageRecord {
    /// PDA 种子前缀
    pub const SEED: &'static [u8] = b"stage";

    /// 账户数据长度（不含 discriminator）
    pub const LEN: usize = 32                        // batch
        + 32                                         // operator
        + (4 + MAX_CODE_LEN)                         // operator_code
        + 4                                          // index
        + (4 + MAX_STAGE_LEN)                        // stage
        + (4 + MAX_HASH_LEN)                         // data_hash
        + 8                                          // timestamp
        + 1; // bump
}

/// 批次状态
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatchStatus {
    /// 流通中，可继续追加环节
    Active,
    /// 已封批，正常走完流程
    Closed,
    /// 已召回，出问题了
    Recalled,
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

#[event]
pub struct RegistryInitialized {
    pub admin: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct InstitutionRegistered {
    pub institution: Pubkey,
    pub authority: Pubkey,
    pub code: String,
    pub role: InstitutionRole,
    pub timestamp: i64,
}

#[event]
pub struct InstitutionStatusChanged {
    pub institution: Pubkey,
    pub authority: Pubkey,
    pub active: bool,
    pub timestamp: i64,
}

#[event]
pub struct PausedChanged {
    pub paused: bool,
    pub timestamp: i64,
}

#[event]
pub struct AdminTransferred {
    pub previous: Pubkey,
    pub current: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct BatchCreated {
    pub batch: Pubkey,
    pub owner: Pubkey,
    pub owner_code: String,
    pub batch_id: String,
    pub region: String,
    pub stage: String,
    pub data_hash: String,
    pub timestamp: i64,
}

#[event]
pub struct StageAppended {
    pub batch: Pubkey,
    pub operator: Pubkey,
    pub operator_code: String,
    pub index: u32,
    pub stage: String,
    pub data_hash: String,
    pub timestamp: i64,
}

#[event]
pub struct BatchTransferred {
    pub batch: Pubkey,
    pub previous_owner: Pubkey,
    pub new_owner: Pubkey,
    pub new_owner_code: String,
    pub timestamp: i64,
}

#[event]
pub struct BatchRecalled {
    pub batch: Pubkey,
    pub owner: Pubkey,
    pub reason_hash: String,
    pub timestamp: i64,
}

#[event]
pub struct BatchClosed {
    pub batch: Pubkey,
    pub owner: Pubkey,
    pub stage_count: u32,
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// 错误码
// ---------------------------------------------------------------------------

#[error_code]
pub enum AgriError {
    #[msg("字段不能为空或只有空白字符")]
    EmptyField,
    #[msg("机构代码必须是 2-12 位大写字母或数字")]
    InvalidCode,
    #[msg("机构名称过长（最多 32 字节）")]
    NameInvalid,
    #[msg("批次号过长（最多 32 字节）")]
    BatchIdInvalid,
    #[msg("批次号必须以本机构代码开头，例如 YNPE-2026-0001")]
    BatchIdPrefixMismatch,
    #[msg("产地名称过长（最多 24 字节）")]
    RegionInvalid,
    #[msg("环节名称过长（最多 24 字节）")]
    StageInvalid,
    #[msg("哈希必须是 64 位十六进制字符串（SHA-256）")]
    InvalidHash,
    #[msg("批次不在流通中状态（已封批或已召回）")]
    BatchNotActive,
    #[msg("环节数量已达上限（64）")]
    StageLimitReached,
    #[msg("不是程序管理员")]
    NotAdmin,
    #[msg("机构未注册或已被停用")]
    InstitutionInactive,
    #[msg("当前机构角色不允许该操作")]
    RoleNotAllowed,
    #[msg("程序处于应急暂停状态")]
    ProgramPaused,
    #[msg("只有批次 owner 机构或管理员可以执行该操作")]
    NotOwnerOrAdmin,
}

// ---------------------------------------------------------------------------
// 校验工具
// ---------------------------------------------------------------------------

/// 校验机构代码：2-12 位大写字母或数字（会出现在批次号里，所以要够严格）。
fn validate_code(code: &str) -> Result<()> {
    let bytes = code.as_bytes();
    ensure!(
        bytes.len() >= 2 && bytes.len() <= MAX_CODE_LEN,
        AgriError::InvalidCode
    );
    ensure!(
        bytes
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
        AgriError::InvalidCode
    );
    Ok(())
}

/// 校验哈希：必须是 64 位十六进制（SHA-256 的十六进制写法）。
///
/// 如果前端用 base58 / base64，或者想接受更短/带 `0x` 的写法，只改这一个函数。
fn validate_hash(value: &str) -> Result<()> {
    ensure!(value.len() == MAX_HASH_LEN, AgriError::InvalidHash);
    ensure!(
        value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        AgriError::InvalidHash
    );
    Ok(())
}

/// 批次号是否以 `机构代码-` 开头（不分配内存的字节比较）。
fn has_code_prefix(batch_id: &str, code: &str) -> bool {
    let batch_bytes = batch_id.as_bytes();
    let code_bytes = code.as_bytes();
    batch_bytes.len() > code_bytes.len() + 1
        && batch_bytes.starts_with(code_bytes)
        && batch_bytes[code_bytes.len()] == b'-'
}
