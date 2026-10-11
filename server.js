// server.js - AgriTrace 后端与Solana链交互演示
// 目标运行环境：Solana Localnet (solana-test-validator)
// RPC: http://127.0.0.1:8899
// 注意：Solana Playground Playnet不对外暴露RPC接口，本代码展示设计思路

const { Connection, PublicKey } = require('@solana/web3.js');
const { Program } = require('@coral-xyz/anchor');

// 👉部署后把真实program id填在这里
const PROGRAM_ID = new PublicKey("PUT_YOUR_PROGRAM_ID_HERE");
const RPC_ENDPOINT = "http://127.0.0.1:8899";

// 模拟从农业采集设备获得的数据
const farmStageData = {
  parentBatch: "HENAN‑AG‑2026‑001",
  operator: "Xinxiang Demo Farm",
  stage: "planting",
  dataHash: "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8"
};

async function runDemo() {
  console.log("=== AgriTrace 后端链交互演示 ===");
  console.log("🔌尝试连接本地Solana链：", RPC_ENDPOINT);

  const conn = new Connection(RPC_ENDPOINT, "confirmed");

  console.log("📋待上链存证数据：", farmStageData);
  console.log("💡设计思路：");
  console.log("   1. 田间IoT系统采集生产信息 → 计算哈希");
  console.log("   2. 后端直接构造交易（无需浏览器钱包）");
  console.log("   3. 发送到Localnet上链，生成不可篡改记录");
  console.log("ℹ️ 注意：执行 `anchor build` 可从lib.rs生成IDL，用于初始化Program对象");
  console.log("✅架构验证完成（仅演示，未实际发送交易）");
}

runDemo().catch(err => {
  console.error("❌发生错误：", err.message);
});