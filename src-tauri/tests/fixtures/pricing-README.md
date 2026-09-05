# 定价目录测试夹具

- `pricing-catalog.json` / `.sig`：签名测试目录（sequence 2099123100，必须**高于**内置目录序号，否则刷新测试会触发拒绝降级）；
- `pricing-test.key` / `.pub`：**测试专用** minisign 密钥对（仅签本夹具，与生产更新/定价密钥无关，允许入库）。

## 修改夹具后重签

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY_PATH = Resolve-Path src-tauri/tests/fixtures/pricing-test.key
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''
npm run tauri signer sign src-tauri/tests/fixtures/pricing-catalog.json
```

## 同步代码

- `src-tauri/src/pricing.rs` → `TEST_PUBLIC_KEY` 必须与 `pricing-test.key.pub` 一致；
- 断言中的 `catalogVersion`（test.2099.12.31.0）随夹具更新。
