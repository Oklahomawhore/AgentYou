# 本地服务 0.2（历史记录）

> 本文保留早期服务实现，模型、工具与调度描述部分已过时。当前架构和验证状态以 [阶段总结](STAGE-SUMMARY-2026-09-23.md) 为准。

## 运行结构

- `crates/mind-runtime`：单写者人格事件内核，继续使用可替换 DecisionProvider 和保守 revision 校验。
- `crates/yourself-server/src/openrouter.rs`：真实 HTTPS OpenRouter 客户端、结构化评估、工具调用、来源注解保留、超时和调用账目。
- `data.rs`：本地应用 SQLite、任务队列、会话、记忆依赖、配置、调用预约、原子消息投递。
- `service.rs`：Axum API、权限控制面、串行对话轮次、后台工作调度、主动通知二次校验。
- `web/`：无 CDN、无前端构建步骤的工作台。页面和静态资源编译进服务程序。
- `scripts/service.py`：仅管理当前仓库对应的服务进程；启动、健康检查、状态、停止。

一个人格只有一个 MindRuntime 写者。应用数据库的所有写入由进程内 Mutex 串行化，关键提交与控制面再以异步锁排序。网络调用不持有数据库锁。应用 SQLite 与人格 SQLite 分工明确：前者保留用户工作台状态，后者保留人格事件及策略状态。

## OpenRouter

固定 `https://openrouter.ai/api/v1/chat/completions`，Bearer Key，拒绝 HTTP 重定向。模型目录来自公开 `/models`。设置表保存 Key，但公开设置响应只返回 `has_key`；不返回 Key 的任何片段。

默认对话/工作和评估均为 `openai/gpt-4.1-mini`。聊天使用受限 function tools：remember / create_task / search_memory。每轮最多 4 次模型请求，前三轮可以调用工具，每次最多 4 个工具。最后一轮只生成文字。工具没有修改权限、执行 Shell 或任意出网的能力。

主动评估使用严格 JSON Schema，独立评分 benefit / novelty / interruption / interest / goal_congruence / surprise / evidence_sufficient；Runtime 校验字段、数值范围、rubric 和快照时效。这是结构化生成模型评估，不冒称 Jev 或校准概率。

模型目录 / 参数能力可能变化。用户选择不支持工具或结构化输出的模型时，服务报错并要求调整，不悄悄降级为不受约束的执行。

配置联网搜索后使用 `web` 插件，最多 3 个结果。聊天与 research 任务启用搜索，反思与评估不启用。回传的 `url_citation` 来源会追加保留；界面仅把 http(s) URL 渲染为外链，其余文本转义显示。

官方接口依据：

- [API 请求 / 响应](https://openrouter.ai/docs/api_reference/overview)
- [API 身份验证](https://openrouter.ai/docs/api_reference/authentication)
- [结构化输出](https://openrouter.ai/docs/guides/features/structured-outputs)
- [Web 搜索与引用注解](https://openrouter.ai/docs/guides/features/plugins/web-search)

## 对话、取消与恢复

客户端为每条消息生成 UUID，服务用它保证重复提交不重复发起对话。每次对话记录用户消息和 pending 回复，再异步请求模型。单次只允许一个对话轮次；后台工作可并行，但不会在对话轮次中投递主动消息。

停止生成通过 watch 信号取消本地请求 Future，立即释放对话轮次；未获得回执的调用被记录为 unknown。暂停、更新配置和删除记忆改变授权版本，旧结果在提交前被丢弃。已开始的供应商执行和费用无法撤回。

重启将 pending 回复、running 工作标记失败；running 模型调用标记 unknown。不自动重试可能已经收费的工作。未开始的 queued 工作保留。持续探索的上次调度时间保存在 SQLite 中，重启不立即重复创建同一个周期的任务。

## 记忆删除传播

每条生成回复、反思和工作记录其实际使用的消息/记忆 ID。删除记忆时遍历依赖关系，删除派生记忆与回复，清空关联工作目标和成果。未完成请求因版本变化不能将旧内容写回。

跨数据库删除使用持久化 `pending_erasure` 清单：先在应用事务中删除并登记，随后清除 MindRuntime 对应事件与决策，成功后清除清单。若中途崩溃，下次启动在恢复人格事件之前先完成清理。模拟情绪为依赖事件的总结，删除时保守重置为基线。

该机制是逻辑删除和上下文撤销，并不是供应商删除 API 或文件系统取证擦除。

## 投递与预算

主动消息经过 Runtime 决策 → 通知草稿生成 → 再检查授权版本、人格 revision、对话轮次及免打扰。四种类别独立 consent / quota / cooldown。当前候选来自真实工作结果、显式好奇/问候任务和预授权持续探索。

页面内消息是唯一投递目标。Outbox 的 READY / SENDING / SENT 与消息插入在同一 SQLite 事务中完成；SENT 意味着工作台消息已经持久化，不等于用户已读。event_id 唯一约束防止重复投递。外部消息渠道尚未实现，不能套用这里的原子提交来声称外部 exactly-once。

主动决策内核仍使用 shadow 标记表示“策略建议”，服务层对通过再次授权的建议执行真实本地投递。内核 Speak 预占模拟主动配额，因此生成失败或丢弃也可能消耗该主动配额；这是保守行为。

调用次数先预约后出网，过去 24 小时达到额度则拒绝；取消和未知请求也占额度。token / cost 来自接口回执，未返回值保留未知。不是美元硬预算，不包含独立核验供应商账单。

## 验证范围

- 原有 Runtime 与 CLI：15 项 Rust 测试。
- 服务：OpenRouter 请求契约、实际 HTTP 工具往返、幂等、Key 脱敏、CSRF/Host 校验、缺 Key/暂停/配额、错误回执、反思和主动投递、取消、依赖删除、重启恢复、引用保留。
- 原有 Python 骨架：29 项测试。
- 浏览器：首屏、配置入口、公开模型列表、任务入口、桌面和 390px 窄屏、控制台错误检查。

没有真实 Key，因此没有声称已验证付费模型回复质量、全部 OpenRouter 模型兼容性或搜索结果真实性。默认模型的真实连接测试留给用户在设置页完成。
