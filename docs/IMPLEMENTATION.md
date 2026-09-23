# 阶段 A 实现记录

> 此文件保留第一阶段的历史实现范围。当前实现状态见 [阶段总结](STAGE-SUMMARY-2026-09-23.md)，不再仅有离线 Mock。

## 范围与边界

输入设计文档和 `reference/` 作为架构与行为参考。仓库原为空，没有可以直接接入的 Rust Main/Work 或 ToolRuntime，因此本次创建独立 workspace 和 `mind-runtime` crate。

第一版完整运行路径：可信宿主提交 Event → 入库与去重 → 类别硬门禁 → 不可变证据快照 → 离线 Provider → 状态复查 → 情绪 reducer / 策略 → 原子记录影子决策。

这不是完整桌面产品。Rust 目前没有 HTTP、LLM、OS 读取或消息发送实现。原参考包的 Python HTTP adapter 仅保留，不代表已验证线上 API。所有 Rust 决策携带 `shadow: true`；`respond` 代表应交给 Main，`reflect` 代表值得进一步核实，均未实际执行。

## 持久化与恢复

- `meta`：schema、persona、guards、revision、affect。
- `inbox`：完整授权事件及 pending / processing / done 状态。
- `decisions`：最终决策、原始 appraisal、模型与策略版本、候选 ID、类别、时间。
- 决策插入、情绪更新及 inbox 完成在同一事务内。模型等待不持有 SQLite 事务。
- 启动时把 processing 转为 pending；单 actor 按入库顺序恢复。恢复会重新检查当前权限、证据和到期时间。未提交的只读模型请求可以重做；未来远程 Provider 因崩溃重做可能产生额外费用。
- 主动候选 Speak 有唯一索引；已完成事件不会重放。完成的 WAIT/STALE 是审计终态，后续重新评估需宿主提交新的事件 ID。
- 影子 Speak 占用模拟配额，避免在回放中每个候选都被判定为可说。不会创建真实 Outbox。
- 文件锁在启动恢复之前获取，并在 shutdown 回执前释放。只支持本地单实例文件，不承诺网络文件系统、硬链接别名或分布式调度。

## 与方案 / Python 参考的明确差异

1. Rust 首版用滚动 24 小时窗口实现每类配额，比跨午夜重置更保守；尚未实现按本地自然日重置和时区配置。
2. 四类 consent / limit / cooldown 独立。当前语义评分权重共用参考策略，后续需要分类别校准；不是四套经过验证的效用模型。
3. 全局 revision 保守取消过时评估，尚未实现 perception / dialogue / permission 分区 epoch 和饥饿监测。
4. Rust 首版所有行为都是影子决策。Python 的发送状态机只作为参考，不能据此声称 Rust 已实现 READY → SENDING → SENT / UNKNOWN。
5. 事件的 `source` 是可信接入方提供的标识，不是身份认证。字段级清洗、连接器认证和供应商数据策略必须在接入真实来源前实现。
6. 检查 evidence ID 和有效期不能证明候选陈述语义为真。当前记录显式模拟情绪，不记录私有思维链或虚构经历。
7. 数据库目前保存原始授权文本，没有加密、保留期和删除传播；仅用于离线合成数据开发。
8. Persona 配置固定且持久化；Beliefs、Drives、Goals、分层 Memory 和 SelfModel 的完整 reducer 留在阶段 C。

## 后续可接入点

- `DecisionProvider`：替换 Mock；远程实现须声明 `remote()`，返回固定 model / rubric 标识。runtime 校验范围和 rubric；真实适配器还需校验指定 model 版本与 HTTP 响应契约。
- `Runtime::submit`：宿主鉴权、数据最小化后的事件入口；不得把该入口直接开放给不可信 WebView。
- `Runtime::set_guards`：可信宿主控制面，禁止作为 LLM 可调用工具。
- `Decision`：阶段 B 的 Main / Work 路由输入。不能直接把 `Speak` 当作发送授权，仍需草稿证据检查、epoch 复查和 Outbox 回执。

## 验证

Rust 集成测试覆盖影子决策、事件 / 候选幂等、类别 consent、直接对话、暂停、未知来源不落库、权限变更不阻塞、过期证据、云权限、异常概率、反思提案、持久化配额、恢复、模型超时、写者互斥和情绪行为差异。

原有 29 项 Python unittest 在本机 Python 3.14.7 上通过。Rust 测试和回放输出只证明当前离线契约，不证明真实模型的语义质量或线上发送安全性。
