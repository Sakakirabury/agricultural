// Solana Playground · client/client.ts
// 使用方式：在 Solana Playground 左侧 Client 面板点 Run。
// 不需要任何 import —— web3 / anchor / pg 在 Playground 里都是全局可用的。
//
// 这一版走的是**加固后的完整流程**：
//   初始化注册表 → 注册机构（生产方 + 一个物流方）→ 建批次 → 追加环节
//   → 封批并验证拒绝写入 → 再建一个批次 → 转让给物流方 → admin 召回 → 验证拒绝写入

const program = pg.program;
const wallet = pg.wallet;

// 机构代码会作为批次号前缀（防抢注），所以必须是 2-12 位大写字母/数字
const ORG_CODE = "YNPE";
const ORG_NAME = "云南普洱农场";

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/** 真实计算 SHA-256，返回 64 位小写十六进制（和链上 validate_hash 的要求一致）。 */
async function sha256Hex(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

const registryPda = (): web3.PublicKey =>
  web3.PublicKey.findProgramAddressSync([Buffer.from("registry")], program.programId)[0];

const institutionPda = (authority: web3.PublicKey): web3.PublicKey =>
  web3.PublicKey.findProgramAddressSync(
    [Buffer.from("institution"), authority.toBuffer()],
    program.programId
  )[0];

/** 机构代码占用登记 PDA：同一个代码只能被注册一次 */
const codeClaimPda = (code: string): web3.PublicKey =>
  web3.PublicKey.findProgramAddressSync(
    [Buffer.from("code"), Buffer.from(code, "utf8")],
    program.programId
  )[0];

const batchPdaOf = (batchId: string): web3.PublicKey =>
  web3.PublicKey.findProgramAddressSync(
    [Buffer.from("batch"), Buffer.from(batchId, "utf8")],
    program.programId
  )[0];

const stagePdaOf = (batch: web3.PublicKey, index: number): web3.PublicKey =>
  web3.PublicKey.findProgramAddressSync(
    [Buffer.from("stage"), batch.toBuffer(), new anchor.BN(index).toArrayLike(Buffer, "le", 4)],
    program.programId
  )[0];

const short = (key: web3.PublicKey) => `${key.toBase58().slice(0, 6)}…${key.toBase58().slice(-4)}`;
const isActive = (status: any) => "active" in status;
const statusName = (status: any) =>
  "active" in status ? "流通中" : "closed" in status ? "已封批" : "已召回";

// ---------------------------------------------------------------------------
// 治理：注册表与机构
// ---------------------------------------------------------------------------

async function ensureRegistry() {
  const pda = registryPda();
  const existing = await program.account.registry.fetchNullable(pda);
  if (existing) {
    console.log(`  注册表已存在，admin = ${short(existing.admin)}，机构数 ${existing.institutionCount}`);
    return existing;
  }
  await program.methods
    .initializeRegistry()
    .accounts({ registry: pda, admin: wallet.publicKey, systemProgram: web3.SystemProgram.programId })
    .rpc();
  console.log(`  注册表已初始化，admin = ${short(wallet.publicKey)}`);
  return program.account.registry.fetch(pda);
}

async function ensureInstitution(
  authority: web3.Keypair | typeof wallet,
  code: string,
  name: string,
  role: any
) {
  const pda = institutionPda(authority.publicKey);
  const existing = await program.account.institution.fetchNullable(pda);
  if (existing) {
    console.log(`  机构 ${existing.code}（${existing.name}）已注册`);
    return existing;
  }
  const builder = program.methods
    .registerInstitution(code, name, role)
    .accounts({
      registry: registryPda(),
      codeClaim: codeClaimPda(code),
      institution: pda,
      authority: authority.publicKey,
      admin: wallet.publicKey,
      systemProgram: web3.SystemProgram.programId,
    });
  // 机构操作员也要签名（证明私钥在自己手里）；只有 keypair 需要额外签
  if ("secretKey" in authority) builder.signers([authority as web3.Keypair]);
  await builder.rpc();
  const created = await program.account.institution.fetch(pda);
  console.log(`  机构已注册：${created.code}（${created.name}）角色 ${JSON.stringify(created.role)}`);
  return created;
}

// ---------------------------------------------------------------------------
// 演示
// ---------------------------------------------------------------------------

async function main() {
  console.log("=== AgriTrace 演示（加固版）===");
  console.log("钱包:", wallet.publicKey.toBase58(), "\n");

  console.log("[0] 治理准备");
  await ensureRegistry();
  const producer = await ensureInstitution(wallet, ORG_CODE, ORG_NAME, { producer: {} });

  // 再注册一个物流机构（用新生成的 keypair，演示批次转让的接收方）
  // 代码占用登记是全局唯一的，所以这里每次跑都用带时间戳的代码，避免和上次冲突
  const runTag = Date.now().toString().slice(-4);
  const logistics = web3.Keypair.generate();
  const logisticsInst = await ensureInstitution(
    logistics,
    `YNWL${runTag}`,
    "云南冷链物流",
    { logistics: {} }
  );
  console.log(`  物流机构地址 ${short(logistics.publicKey)}（本次演示不要求它出钱，只演示转让）\n`);

  // ---------------------------------------------------------------- 批次 A
  const batchA = `${ORG_CODE}-2026-0001`;
  const pdaA = batchPdaOf(batchA);
  console.log(`[1] 批次 A：${batchA}`);
  if (await program.account.batchRecord.fetchNullable(pdaA)) {
    console.log("  已存在，跳过创建");
  } else {
    await program.methods
      .createBatch(
        batchA,
        "云南·普洱",
        "播种",
        await sha256Hex("BATCH-A|播种|2026-10-11T08:00:00+08:00|地块A-03")
      )
      .accounts({
        registry: registryPda(),
        institution: institutionPda(wallet.publicKey),
        batchRecord: pdaA,
        authority: wallet.publicKey,
        systemProgram: web3.SystemProgram.programId,
      })
      .rpc();
    console.log("  建批次 ✓（批次号以机构代码开头，链上校验通过）");
  }

  for (const [name, payload] of [
    ["施肥", "BATCH-A|施肥|2026-10-18T09:30:00+08:00|有机肥120kg"],
    ["采收", "BATCH-A|采收|2026-11-05T07:15:00+08:00|鲜叶86.4kg"],
    ["运输", "BATCH-A|运输|2026-11-06T14:00:00+08:00|冷链车云A12345"],
  ] as [string, string][]) {
    const batch = await program.account.batchRecord.fetch(pdaA);
    const index = batch.stageCount.toNumber();
    await program.methods
      .appendStage(name, await sha256Hex(payload))
      .accounts({
        registry: registryPda(),
        institution: institutionPda(wallet.publicKey),
        batchRecord: pdaA,
        stageRecord: stagePdaOf(pdaA, index),
        authority: wallet.publicKey,
        systemProgram: web3.SystemProgram.programId,
      })
      .rpc();
    console.log(`  追加环节 #${index} ${name} ✓`);
  }

  // 封批（只有 owner 能调）
  await program.methods
    .closeBatch()
    .accounts({ batchRecord: pdaA, authority: wallet.publicKey })
    .rpc();
  console.log("[2] 封批 ✓");

  // 封批后再追加 → 必须被拒
  const closedBatch = await program.account.batchRecord.fetch(pdaA);
  try {
    await program.methods
      .appendStage("偷加一环", await sha256Hex("after-close"))
      .accounts({
        registry: registryPda(),
        institution: institutionPda(wallet.publicKey),
        batchRecord: pdaA,
        stageRecord: stagePdaOf(pdaA, closedBatch.stageCount.toNumber()),
        authority: wallet.publicKey,
        systemProgram: web3.SystemProgram.programId,
      })
      .rpc();
    console.log("  ⚠️ 意外成功");
  } catch (err) {
    console.log("  封批后写入被拒 ✓ ——", (err as any)?.error?.errorCode?.code);
  }

  // ---------------------------------------------------------------- 批次 B
  const batchB = `${ORG_CODE}-2026-0002`;
  const pdaB = batchPdaOf(batchB);
  console.log(`\n[3] 批次 B：${batchB}（用来演示转让与召回）`);
  if (!(await program.account.batchRecord.fetchNullable(pdaB))) {
    await program.methods
      .createBatch(batchB, "云南·临沧", "播种", await sha256Hex("BATCH-B|播种|2026-10-12T08:00:00+08:00"))
      .accounts({
        registry: registryPda(),
        institution: institutionPda(wallet.publicKey),
        batchRecord: pdaB,
        authority: wallet.publicKey,
        systemProgram: web3.SystemProgram.programId,
      })
      .rpc();
    console.log("  建批次 ✓");
  }

  // 转让给物流机构（当前 owner 签名即可，接收方不用签名）
  const beforeTransfer = await program.account.batchRecord.fetch(pdaB);
  if (isActive(beforeTransfer.status) && beforeTransfer.owner.equals(wallet.publicKey)) {
    await program.methods
      .transferBatch()
      .accounts({
        registry: registryPda(),
        newOwnerAuthority: logistics.publicKey,
        newOwnerInstitution: institutionPda(logistics.publicKey),
        batchRecord: pdaB,
        authority: wallet.publicKey,
      })
      .rpc();
    const after = await program.account.batchRecord.fetch(pdaB);
    console.log(`  转让 ✓ 新 owner = ${after.ownerCode}（${short(after.owner)}）`);

    // 转让后我们不再是 owner → 封批必须被拒
    try {
      await program.methods
        .closeBatch()
        .accounts({ batchRecord: pdaB, authority: wallet.publicKey })
        .rpc();
      console.log("  ⚠️ 意外成功：非 owner 也能封批");
    } catch (err) {
      console.log("  非 owner 封批被拒 ✓ ——", (err as any)?.error?.errorCode?.code);
    }
  }

  // admin 召回（admin 与 owner 都允许，这里演示 admin 通道）
  const beforeRecall = await program.account.batchRecord.fetch(pdaB);
  if (isActive(beforeRecall.status)) {
    await program.methods
      .recallBatch(await sha256Hex("BATCH-B|召回|2026-11-20|抽检不合格|报告 RPT-88"))
      .accounts({
        registry: registryPda(),
        batchRecord: pdaB,
        authority: wallet.publicKey,
      })
      .rpc();
    console.log("  召回 ✓（原因哈希已上链留痕）");
  }

  try {
    const recalled = await program.account.batchRecord.fetch(pdaB);
    await program.methods
      .appendStage("召回后追加", await sha256Hex("after-recall"))
      .accounts({
        registry: registryPda(),
        institution: institutionPda(wallet.publicKey),
        batchRecord: pdaB,
        stageRecord: stagePdaOf(pdaB, recalled.stageCount.toNumber()),
        authority: wallet.publicKey,
        systemProgram: web3.SystemProgram.programId,
      })
      .rpc();
    console.log("  ⚠️ 意外成功：召回后仍能写入");
  } catch (err) {
    console.log("  召回后写入被拒 ✓ ——", (err as any)?.error?.errorCode?.code);
  }

  // ---------------------------------------------------------------- 读回
  console.log("\n=== 链上数据 ===");
  for (const [label, pda] of [["A", pdaA], ["B", pdaB]] as [string, web3.PublicKey][]) {
    const b = await program.account.batchRecord.fetch(pda);
    console.log(
      `批次${label} ${b.batchId}  产地 ${b.region}  owner ${b.ownerCode}  ` +
        `状态 ${statusName(b.status)}  环节数 ${b.stageCount.toNumber()}`
    );
    for (let i = 0; i < b.stageCount.toNumber(); i++) {
      const rec = await program.account.stageRecord.fetch(stagePdaOf(pda, i));
      console.log(
        `   #${rec.index.toNumber()} ${rec.stage.padEnd(4, "　")} ${rec.operatorCode}  ` +
          `${new Date(rec.timestamp.toNumber() * 1000).toLocaleString()}  ${rec.dataHash.slice(0, 12)}…`
      );
    }
  }
  const producerNow = await program.account.institution.fetch(institutionPda(wallet.publicKey));
  console.log(`\n生产方 ${producerNow.code} 累计建批次数：${producerNow.batchCount.toString()}`);
  console.log(`物流机构 ${logisticsInst.code}（${logisticsInst.name}）已就绪，可作为后续转让的接收方。`);
}

main().then(
  () => console.log("\n完成。"),
  (err) => console.error("\n失败：", err)
);
