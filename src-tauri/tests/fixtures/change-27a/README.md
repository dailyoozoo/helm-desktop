# 变更-27A SQLite 迁移演练库

这些 SQL 只包含合成数据，用于冻结 schema v21 的三类输入基线。它们不是生产数据库
备份，也不包含 Provider 名称、密钥、认证文件或用户提示词。

| 文件 | 用途 | 预期起点 |
| --- | --- | --- |
| `v21-fresh.sql` | v21 新库与最小完整 Turn | `PRAGMA user_version = 21` |
| `v19-sequential-upgrade.sql` | 从 v19 经 v20、v21 逐级升级 | `PRAGMA user_version = 19` |
| `legacy-missing-attribution.sql` | 缺 `turn_id`、Provider、Model 归属的旧数据 | `PRAGMA user_version = 21` |

执行规则：

1. 每次演练复制 SQL 到临时数据库，不原地修改 fixture。
2. 由当前 `SessionHistoryStore` 打开数据库并执行迁移/修复。
3. 检查 `PRAGMA user_version = 21`、`PRAGMA foreign_key_check` 为空、重复打开幂等。
4. legacy 空值保持可识别，禁止按当前 Binding 或时间猜填 Provider/Model/Turn。
5. 后续切片新增 schema 时保留这三类输入，并从 v21 逐级演练到最新版本。

当前自动化入口与覆盖证据见 `docs/变更-27A-基线与验收记录.md`。
