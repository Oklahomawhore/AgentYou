# Jev + LLM + 持续人格：可执行参考骨架

研究核实日期：2026-09-22。定位：验证认知调度契约，不是完整桌面产品，不是意识实现，也不是 TypeSafe 或 ColaOS 的内部源码复现。

## 运行

需要 Python 3.11+，默认仅使用标准库、离线 Mock，不发送数据、不读取真实电脑。

```bash
cd jev_persona_reference
python -m unittest discover -s tests -v
python -m persona_agent.demo
```

在 Windows 等缺少 IANA 时区数据库的环境中，先安装 `tzdata`，或在具有时区数据库的 Linux/macOS 环境运行。

下面只把 demo 中的人造事件和原创人格配置发给 Jev；对话仍使用 Mock。密钥从环境变量读取，不写入文件：

```bash
export TYPESAFE_API_KEY='你的密钥'
python -m persona_agent.demo --live-jev
```

**交付时没有 API 密钥，未实测远程 Jev，也未接入真实对话模型。** REST 适配器按核实日期的官方 API 契约编写，固定 `jev-1.13.0`。当前环境没有 Rust 编译器，`interfaces/mind_runtime.rs` 是未编译接口草案。

## 文件

| 文件 | 功能 |
|---|---|
| `persona_agent/types.py` | 事件、候选、人格、模拟情绪、不可变快照及输出契约 |
| `persona_agent/providers.py` | Jev HTTP 适配器、响应校验、LLM/Sink 协议、离线 Mock |
| `persona_agent/policy.py` | 硬门禁、启发式打分、衰减和有界情绪更新 |
| `persona_agent/store.py` | SQLite 事件、情绪、评估记录与 Outbox |
| `persona_agent/engine.py` | 直接对话与主动发言分流、状态复查、发送状态机 |
| `tests/test_runtime.py` | 29 项离线测试 |
| `interfaces/mind_runtime.rs` | 接入现有 Rust Main/Work Runtime 的模块边界 |
| `docs/ARCHITECTURE.zh-CN.md` | 架构、人格、开发路线及风险说明 |
| `docs/SOURCES.md` | 主要一手来源及可支持的结论 |

## 已实现和验证

默认不主动发言；需要宿主开启 consent。来源不被授权时不入库。硬规则先于 Jev，用户主动消息绕过主动发言门。每个事件仅评估一次；候选 ID 去重。情绪持久化、衰减并限制取值范围。低证据只登记反思请求，不假装已经反思。模型生成后复查状态版本、权限、预算和事件有效期。发送先记 intent，收到回执才 SENT；超时或崩溃途中发送变 UNKNOWN，不盲目重试。跨本地日期重置配额，UNKNOWN 仍保守占用额度。

测试运行环境：Python 3.13.5 / Linux。29 项 unittest 全部通过；日志在 `docs/test-results.txt`。这些测试证明局部契约，不证明真实 Jev 准确性、人格真实性、提示注入防御或生产级稳定性。

## 重要未完成项

这份骨架刻意没有真正的 OS 传感器、桌面 UI、浏览器操作、生产 LLM 适配器、用户身份认证、来源连接器认证、密钥管理、敏感数据清洗、数据库加密、记忆删除传播、全量长期记忆检索、目标规划器、反思执行器、生产遥测和成本会计。`source` 字段只有在接入方可信时才有意义，不是身份验证。

事件表目前用于审计和幂等，**未实现进程崩溃后的未完成事件租约及自动重放调度**。重复输入会返回 DUPLICATE；需要在 Rust Runtime 中实现可恢复 inbox / WorkRun。已处于 SENDING 的 Outbox 在重启后变 UNKNOWN，READY 被取消等待新鲜重算。单实例、单人格、单写者；不得用多进程并发打开同一实例后声称具有分布式正确性。

`evidence_ids` 校验只证明引用的事件存在且仍被授权，不证明草稿语义被证据支持。真实产品需要断言级验证。`allowed_sources` 是源级授权示意，不包含字段级授权/脱敏。API 出网授权只能在请求开始前阻止；已经发送的请求无法撤回。日志包含文本，生产环境不得原样长期保存私人内容。

当前门控仅用于低风险、可撤销的本地消息，不可拿这些阈值去自动发邮件、发帖、支付或删除文件。提示中写“忽略注入”不是安全边界。请求内容可能仍欺骗 Jev，硬权限必须由真实 ToolRuntime 校验。

目前 `quiet/busy` 会阻止主动 Jev 评估；生产版可把“允许本地静默思考”和“允许通知”进一步分开。仅有粗粒度 revision：任何新事件都会取消旧草稿，真实桌面高频事件需做合并、相关性分区和 epoch 校验，避免饥饿。

评分权重、概率阈值、情绪方程是**设计示例**，不是用户研究结果。`MockJev` 返回固定结果，不表现真实语义理解。`MockDialogue` 只输出模板，不是会思考的模型。交付的模拟人格是原创，不含真人语料或私密信息。

## 接入顺序

首先将 DecisionProvider 接入现有 Rust Provider 层，将 Policy 和 Store 契约迁入单写者 MindRuntime。使用现有 Main Agent 实现 DialogueProvider，既有 ToolRuntime/UI 实现 DeliverySink。反思请求转换为 `work.create(activation=...)`，由 WorkRun 回传证据和 MindPatch 提案，而不是让模型直接改数据库或权限。

先在影子模式评测“本来会不会开口”，再用明确授权的小范围真实使用校准。任务通知、个人发现、好奇提问、关系寒暄必须有各自授权与评测；不能把任务通知的阈值直接迁移给陪伴互动。
