// Solana Playground · tests/agri_trace.ts
//
// 注意：`anchor.workspace.AgriTrace` 里的名字来自项目的 Cargo.toml package name
// （agri_trace → AgriTrace）。如果你的项目名不是 agri_trace，改这一处即可。
//
// 这套用例覆盖：治理（注册表 / 机构 / 角色 / 停用 / 暂停 / 移交）
// 与批次（建批 / 前缀校验 / 追加 / 跳号 / 转让 / 召回 / 封批）。

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AgriTrace } from "../target/types/agri_trace";
import { assert } from "chai";

describe("agri_trace", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.AgriTrace as Program<AgriTrace>;
  const adminPk = provider.wallet.publicKey;

  // 代码占用登记是全局唯一的：测试钱包的机构只注册一次（下面有存在性检查），
  // 其余机构每次跑测试都换一个带时间戳的代码，避免和上一次运行撞车。
  const runTag = Date.now().toString().slice(-4);
  const PRODUCER_CODE = "TESTCO";
  const LOGISTICS_CODE = `YNWL${runTag}`;
  const REGULATOR_CODE = `REGT${runTag}`;
  const FRESH_CODE = `FRESH${runTag}`;

  const HASH_ANY = "a".repeat(64);
  const HASH_OTHER = "b".repeat(64);

  /** null = 用测试钱包（admin / 生产方）；Keypair = 另一个机构 */
  type Who = anchor.web3.Keypair | null;
  const pkOf = (who: Who) => (who === null ? adminPk : who.publicKey);
  const sigOf = (who: Who) => (who === null ? [] : [who]);

  let logistics: anchor.web3.Keypair;
  let regulator: anchor.web3.Keypair;
  let outsider: anchor.web3.Keypair;
  let fresh: anchor.web3.Keypair;

  let seq = 0;
  const newBatchId = () => `${PRODUCER_CODE}-${Date.now()}-${seq++}`;

  const registryPda = () =>
    anchor.web3.PublicKey.findProgramAddressSync([Buffer.from("registry")], program.programId)[0];

  const institutionPda = (authority: anchor.web3.PublicKey) =>
    anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("institution"), authority.toBuffer()],
      program.programId
    )[0];

  /** 机构代码占用登记 PDA：同一个代码只能被注册一次 */
  const codeClaimPda = (code: string) =>
    anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("code"), Buffer.from(code, "utf8")],
      program.programId
    )[0];

  const batchPdaOf = (batchId: string) =>
    anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("batch"), Buffer.from(batchId, "utf8")],
      program.programId
    )[0];

  const stagePdaOf = (batch: anchor.web3.PublicKey, index: number) =>
    anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("stage"), batch.toBuffer(), new anchor.BN(index).toArrayLike(Buffer, "le", 4)],
      program.programId
    )[0];

  // -------------------------------------------------------------- 指令封装

  const createBatch = (batchId: string, who: Who = null, hash = HASH_ANY) =>
    program.methods
      .createBatch(batchId, "云南·普洱", "播种", hash)
      .accounts({
        registry: registryPda(),
        institution: institutionPda(pkOf(who)),
        batchRecord: batchPdaOf(batchId),
        authority: pkOf(who),
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers(sigOf(who))
      .rpc();

  const appendStage = async (batchId: string, who: Who = null, index?: number, hash = HASH_ANY) => {
    const batch = await program.account.batchRecord.fetch(batchPdaOf(batchId));
    const idx = index === undefined ? batch.stageCount.toNumber() : index;
    return program.methods
      .appendStage("施肥", hash)
      .accounts({
        registry: registryPda(),
        institution: institutionPda(pkOf(who)),
        batchRecord: batchPdaOf(batchId),
        stageRecord: stagePdaOf(batchPdaOf(batchId), idx),
        authority: pkOf(who),
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers(sigOf(who))
      .rpc();
  };

  const closeBatch = (batchId: string, who: Who = null) =>
    program.methods
      .closeBatch()
      .accounts({ batchRecord: batchPdaOf(batchId), authority: pkOf(who) })
      .signers(sigOf(who))
      .rpc();

  const recallBatch = (batchId: string, who: Who = null, hash = HASH_ANY) =>
    program.methods
      .recallBatch(hash)
      .accounts({ registry: registryPda(), batchRecord: batchPdaOf(batchId), authority: pkOf(who) })
      .signers(sigOf(who))
      .rpc();

  const transferBatch = (batchId: string, who: Who, newOwnerAuthority: anchor.web3.PublicKey) =>
    program.methods
      .transferBatch()
      .accounts({
        registry: registryPda(),
        newOwnerAuthority,
        newOwnerInstitution: institutionPda(newOwnerAuthority),
        batchRecord: batchPdaOf(batchId),
        authority: pkOf(who),
      })
      .signers(sigOf(who))
      .rpc();

  const registerInstitution = (
    authority: anchor.web3.Keypair | null,
    code: string,
    name: string,
    role: any,
    admin: anchor.web3.Keypair | null = null
  ) => {
    const authorityPk = pkOf(authority);
    // 同一个 keypair 同时当 authority 和 admin 时去重，避免重复 signer
    const extra = [authority, admin].filter((k): k is anchor.web3.Keypair => k !== null);
    const signers = Array.from(new Map(extra.map((k) => [k.publicKey.toBase58(), k])).values());
    return program.methods
      .registerInstitution(code, name, role)
      .accounts({
        registry: registryPda(),
        codeClaim: codeClaimPda(code),
        institution: institutionPda(authorityPk),
        authority: authorityPk,
        admin: pkOf(admin),
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers(signers)
      .rpc();
  };

  const setInstitutionActive = (authority: anchor.web3.PublicKey, active: boolean) =>
    program.methods
      .setInstitutionActive(active)
      .accounts({
        registry: registryPda(),
        authority,
        institution: institutionPda(authority),
        admin: adminPk,
      })
      .rpc();

  const setPaused = (paused: boolean) =>
    program.methods.setPaused(paused).accounts({ registry: registryPda(), admin: adminPk }).rpc();

  // -------------------------------------------------------------- 断言工具

  async function expectError(promise: Promise<unknown>, code: string) {
    let threw = false;
    try {
      await promise;
    } catch (err: any) {
      threw = true;
      const actual =
        err?.error?.errorCode?.code ?? err?.error?.errorMessage ?? err?.message ?? String(err);
      assert.include(String(actual), code, `期望错误 ${code}，实际是 ${actual}`);
    }
    assert.isTrue(threw, `期望调用失败（${code}），但它成功了`);
  }

  async function expectThrow(promise: Promise<unknown>, why: string) {
    let threw = false;
    try {
      await promise;
    } catch {
      threw = true;
    }
    assert.isTrue(threw, `期望失败：${why}`);
  }

  // -------------------------------------------------------------- 准备

  /** 从测试钱包转 SOL 给新 keypair（转账不用 faucet，稳）。 */
  const fund = async (to: anchor.web3.PublicKey, sol = 0.3) => {
    const tx = new anchor.web3.Transaction().add(
      anchor.web3.SystemProgram.transfer({
        fromPubkey: adminPk,
        toPubkey: to,
        lamports: Math.round(sol * anchor.web3.LAMPORTS_PER_SOL),
      })
    );
    await provider.sendAndConfirm(tx);
  };

  before(async () => {
    logistics = anchor.web3.Keypair.generate();
    regulator = anchor.web3.Keypair.generate();
    outsider = anchor.web3.Keypair.generate();
    fresh = anchor.web3.Keypair.generate();
    await fund(logistics.publicKey);
    await fund(regulator.publicKey);
    await fund(outsider.publicKey);

    // 注册表（重复跑测试时不会重复初始化）
    if (!(await program.account.registry.fetchNullable(registryPda()))) {
      await program.methods
        .initializeRegistry()
        .accounts({ registry: registryPda(), admin: adminPk, systemProgram: anchor.web3.SystemProgram.programId })
        .rpc();
    }

    // 生产方（测试钱包自己）
    if (!(await program.account.institution.fetchNullable(institutionPda(adminPk)))) {
      await registerInstitution(null, PRODUCER_CODE, "测试农场", { producer: {} });
    }
    if (!(await program.account.institution.fetchNullable(institutionPda(logistics.publicKey)))) {
      await registerInstitution(logistics, LOGISTICS_CODE, "测试冷链", { logistics: {} });
    }
    if (!(await program.account.institution.fetchNullable(institutionPda(regulator.publicKey)))) {
      await registerInstitution(regulator, REGULATOR_CODE, "测试监管", { regulator: {} });
    }
  });

  // -------------------------------------------------------------- 治理

  it("注册表：admin 正确，且不能重复初始化", async () => {
    const registry = await program.account.registry.fetch(registryPda());
    assert.ok(registry.admin.equals(adminPk));
    assert.isFalse(registry.paused);
    assert.isAtLeast(registry.institutionCount.toNumber(), 3);
    await expectThrow(
      program.methods
        .initializeRegistry()
        .accounts({ registry: registryPda(), admin: adminPk, systemProgram: anchor.web3.SystemProgram.programId })
        .rpc(),
      "注册表是单例，不能重复初始化"
    );
  });

  it("非管理员不能注册机构 → NotAdmin", async () => {
    await expectError(
      registerInstitution(outsider, "HACKER", "冒名机构", { producer: {} }, outsider),
      "NotAdmin"
    );
  });

  it("注册机构：字段正确；代码不合法被拒；代码与机构都不能重复注册", async () => {
    // 非法代码：小写、带连字符、过长（失败后账户会被回滚，所以可以反复用同一个 keypair）
    await expectError(registerInstitution(fresh, "lower", "小写代码", { producer: {} }), "InvalidCode");
    await expectError(registerInstitution(fresh, "AB-CD", "带连字符", { producer: {} }), "InvalidCode");
    await expectError(registerInstitution(fresh, "ABCDEFGHIJKLM", "太长", { producer: {} }), "InvalidCode");

    await registerInstitution(fresh, FRESH_CODE, "新机构", { retailer: {} });
    const inst = await program.account.institution.fetch(institutionPda(fresh.publicKey));
    assert.equal(inst.code, FRESH_CODE);
    assert.equal(inst.name, "新机构");
    assert.deepEqual(inst.role, { retailer: {} });
    assert.isTrue(inst.active);
    assert.equal(inst.batchCount.toNumber(), 0);
    assert.ok(inst.authority.equals(fresh.publicKey));

    // 代码占用登记确实指向这个机构
    const claim = await program.account.codeClaim.fetch(codeClaimPda(FRESH_CODE));
    assert.ok(claim.institution.equals(fresh.publicKey));

    // 同一个 authority 重复注册 → institution PDA 已存在
    await expectThrow(
      registerInstitution(fresh, `${FRESH_CODE}X`.slice(0, 12), "重复", { producer: {} }),
      "同一地址不能重复注册机构"
    );

    // 换一个地址、但用已经被占用的代码 → 代码占用登记 PDA 已存在
    const thief = anchor.web3.Keypair.generate();
    await expectThrow(
      registerInstitution(thief, FRESH_CODE, "抢代码", { producer: {} }),
      "代码已被占用，不能重复注册"
    );
  });

  it("停用机构后不能再建批次 → InstitutionInactive（恢复后可用）", async () => {
    await setInstitutionActive(adminPk, false);
    await expectError(createBatch(newBatchId()), "InstitutionInactive");

    await setInstitutionActive(adminPk, true);
    const batchId = newBatchId();
    await createBatch(batchId);
    assert.equal((await program.account.batchRecord.fetch(batchPdaOf(batchId))).batchId, batchId);
  });

  it("未注册的地址建批次 → 失败", async () => {
    await expectThrow(createBatch(newBatchId(), outsider), "未注册机构写不了数据");
  });

  it("程序应急暂停时拒绝写入 → ProgramPaused", async () => {
    await setPaused(true);
    await expectError(createBatch(newBatchId()), "ProgramPaused");
    await setPaused(false);
    const batchId = newBatchId();
    await createBatch(batchId);
    assert.ok((await program.account.batchRecord.fetch(batchPdaOf(batchId))).batchId === batchId);
  });

  it("移交管理员：新 admin 生效，旧 admin 失去权限", async () => {
    const next = anchor.web3.Keypair.generate();
    await fund(next.publicKey);
    await program.methods
      .transferAdmin()
      .accounts({ registry: registryPda(), admin: adminPk, newAdmin: next.publicKey })
      .signers([next])
      .rpc();
    assert.ok((await program.account.registry.fetch(registryPda())).admin.equals(next.publicKey));

    // 旧 admin 现在不是管理员了
    await expectError(
      program.methods.setPaused(true).accounts({ registry: registryPda(), admin: adminPk }).rpc(),
      "NotAdmin"
    );

    // 还回去（新 admin 必须签名）
    await program.methods
      .transferAdmin()
      .accounts({ registry: registryPda(), admin: next.publicKey, newAdmin: adminPk })
      .signers([next])
      .rpc();
    assert.ok((await program.account.registry.fetch(registryPda())).admin.equals(adminPk));
  });

  // -------------------------------------------------------------- 批次

  it("批次号必须带本机构前缀 → BatchIdPrefixMismatch", async () => {
    await expectError(createBatch("OTHER-2026-0001"), "BatchIdPrefixMismatch");
    // 只写代码、没有连字符也不放过
    await expectError(createBatch(`${PRODUCER_CODE}2026`), "BatchIdPrefixMismatch");
  });

  it("建批次：字段 / 状态 / PDA / 机构计数都正确", async () => {
    const before = await program.account.institution.fetch(institutionPda(adminPk));
    const batchId = newBatchId();
    await createBatch(batchId);
    const batch = await program.account.batchRecord.fetch(batchPdaOf(batchId));

    assert.equal(batch.batchId, batchId);
    assert.equal(batch.region, "云南·普洱");
    assert.equal(batch.stage, "播种");
    assert.equal(batch.dataHash, HASH_ANY);
    assert.equal(batch.ownerCode, PRODUCER_CODE);
    assert.equal(batch.stageCount.toNumber(), 0);
    assert.deepEqual(batch.status, { active: {} });
    assert.ok(batch.owner.equals(adminPk));
    assert.equal(batch.createdAt.toNumber(), batch.updatedAt.toNumber());

    const after = await program.account.institution.fetch(institutionPda(adminPk));
    assert.equal(after.batchCount.toNumber(), before.batchCount.toNumber() + 1);
  });

  it("哈希与长度校验 → InvalidHash / RegionInvalid / StageInvalid / EmptyField", async () => {
    await expectError(createBatch(newBatchId(), null, "not-a-hash"), "InvalidHash");
    await expectError(createBatch(newBatchId(), null, "z".repeat(64)), "InvalidHash");
    await expectError(createBatch(newBatchId(), null, HASH_ANY.slice(0, 63)), "InvalidHash");

    // 自定义字段的建批次调用（region / stage 各来一发）
    const inlineCreate = (batchId: string, region: string, stage: string, hash: string) =>
      program.methods
        .createBatch(batchId, region, stage, hash)
        .accounts({
          registry: registryPda(),
          institution: institutionPda(adminPk),
          batchRecord: batchPdaOf(batchId),
          authority: adminPk,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .rpc();

    const idRegion = newBatchId();
    await expectError(inlineCreate(idRegion, "云".repeat(9), "播种", HASH_ANY), "RegionInvalid");
    const idStage = newBatchId();
    await expectError(inlineCreate(idStage, "云南", "播".repeat(9), HASH_ANY), "StageInvalid");
    const idEmpty = newBatchId();
    await expectError(inlineCreate(idEmpty, "云南", "   ", HASH_ANY), "EmptyField");
  });

  it("追加环节：序号严格递增，并记录操作方机构代码", async () => {
    const batchId = newBatchId();
    await createBatch(batchId);
    await appendStage(batchId, null, 0);
    await appendStage(batchId, null, 1, HASH_OTHER);

    const batch = await program.account.batchRecord.fetch(batchPdaOf(batchId));
    assert.equal(batch.stageCount.toNumber(), 2);

    const zero = await program.account.stageRecord.fetch(stagePdaOf(batchPdaOf(batchId), 0));
    const one = await program.account.stageRecord.fetch(stagePdaOf(batchPdaOf(batchId), 1));
    assert.equal(zero.index.toNumber(), 0);
    assert.equal(zero.operatorCode, PRODUCER_CODE);
    assert.equal(one.index.toNumber(), 1);
    assert.equal(one.dataHash, HASH_OTHER);
    assert.ok(one.operator.equals(adminPk));
  });

  it("序号不能跳号：只有 #0 时直接建 #3 会失败", async () => {
    const batchId = newBatchId();
    await createBatch(batchId);
    await expectThrow(appendStage(batchId, null, 3), "序号跳号");
  });

  it("监管角色只读：不能建批次、也不能追加环节 → RoleNotAllowed", async () => {
    await expectError(createBatch(`${REGULATOR_CODE}-2026-0001`, regulator), "RoleNotAllowed");

    const batchId = newBatchId();
    await createBatch(batchId);
    await expectError(appendStage(batchId, regulator), "RoleNotAllowed");
  });

  it("转让批次：新 owner 接手，旧 owner 失去封批权限", async () => {
    const batchId = newBatchId();
    await createBatch(batchId);
    await appendStage(batchId, null, 0);

    await transferBatch(batchId, null, logistics.publicKey);
    const batch = await program.account.batchRecord.fetch(batchPdaOf(batchId));
    assert.ok(batch.owner.equals(logistics.publicKey));
    assert.equal(batch.ownerCode, LOGISTICS_CODE);

    // 旧 owner（测试钱包）封批 → 拒绝
    await expectError(closeBatch(batchId, null), "NotOwnerOrAdmin");

    // 新 owner 封批 → 成功（它是已充值、已注册的机构）
    await closeBatch(batchId, logistics);
    assert.deepEqual((await program.account.batchRecord.fetch(batchPdaOf(batchId))).status, {
      closed: {},
    });
  });

  it("召回：只有 owner / admin 能召回，召回后拒绝写入", async () => {
    const batchId = newBatchId();
    await createBatch(batchId);

    // 物流机构既不是 owner 也不是 admin → 拒绝
    await expectError(recallBatch(batchId, logistics), "NotOwnerOrAdmin");

    // owner（测试钱包）召回
    await recallBatch(batchId, null, HASH_OTHER);
    const recalled = await program.account.batchRecord.fetch(batchPdaOf(batchId));
    assert.deepEqual(recalled.status, { recalled: {} });
    assert.equal(recalled.recallReasonHash, HASH_OTHER);

    await expectError(appendStage(batchId), "BatchNotActive");
    await expectError(recallBatch(batchId, null), "BatchNotActive");
    await expectError(closeBatch(batchId, null), "BatchNotActive");
  });

  it("封批后拒绝写入 → BatchNotActive", async () => {
    const batchId = newBatchId();
    await createBatch(batchId);
    await appendStage(batchId, null, 0);
    await closeBatch(batchId, null);

    assert.deepEqual((await program.account.batchRecord.fetch(batchPdaOf(batchId))).status, {
      closed: {},
    });
    await expectError(appendStage(batchId), "BatchNotActive");
    await expectError(closeBatch(batchId, null), "BatchNotActive");
  });
});
