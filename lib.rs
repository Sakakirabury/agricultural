use anchor_lang::prelude::*;

declare_id!("2yLWQPC5Jy6bdC3MLUFPucPrbrXP7qgfPBCABAisAsgD");

#[program]
pub mod agri_trace {
    use super::*;

    /// 创建一个新的农产品批次
    pub fn create_batch(
        ctx: Context<CreateBatch>,
        batch_id: String,
        stage: String,
        data_hash: String
    ) -> Result<()> {
        let record = &mut ctx.accounts.batch_record;
        record.owner = ctx.accounts.signer.key();
        record.batch_id = batch_id;
        record.stage = stage;
        record.data_hash = data_hash;
        record.ts = Clock::get()?.unix_timestamp;
        Ok(())
    }

    /// 给已有批次追加一个流通环节记录
    pub fn append_stage(
        ctx: Context<AppendStage>,
        stage: String,
        data_hash: String
    ) -> Result<()> {
        let record = &mut ctx.accounts.stage_record;
        record.parent_batch = ctx.accounts.batch_record.key();
        record.operator = ctx.accounts.signer.key();
        record.stage = stage;
        record.data_hash = data_hash;
        record.ts = Clock::get()?.unix_timestamp;
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(batch_id: String)]
pub struct CreateBatch<'info> {
    #[account(
        init,
        payer = signer,
        space = 8 + 32 + 64 + 32 + 64 + 8
    )]
    pub batch_record: Account<'info, BatchRecord>,

    #[account(mut)]
    pub signer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AppendStage<'info> {
    /// 父批次（只读，用来关联）
    pub batch_record: Account<'info, BatchRecord>,

    #[account(
        init,
        payer = signer,
        space = 8 + 32 + 32 + 64 + 64 + 8
    )]
    pub stage_record: Account<'info, StageRecord>,

    #[account(mut)]
    pub signer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// 主批次账户：记录批次基础信息
#[account]
pub struct BatchRecord {
    pub owner: Pubkey,
    pub batch_id: String,
    pub stage: String,
    pub data_hash: String,
    pub ts: i64,
}

/// 追加环节账户：每一个流通阶段新建一条
#[account]
pub struct StageRecord {
    pub parent_batch: Pubkey,
    pub operator: Pubkey,
    pub stage: String,
    pub data_hash: String,
    pub ts: i64,
}