# Jev 本地诊断 — 2026-09-23

## 文档核对

- https://teamorouter.com/zh/docs/jev-api：POST https://api.teamorouter.com/v1/systemone，Bearer TeamoRouter Key，model=jev，原生 state/questions。
- https://docs.typesafe.ai/api：原生题型 choice / noul / score；直连 TypeSafe 的 jev-latest 与 TeamoRouter 的 jev 别名不能混淆。
- 本地调用未携带 messages、stream、temperature、max_tokens 等聊天接口参数。

## 可复现结果

执行 `python3 scripts/check_jev.py --rounds 2`，使用已有配置 Key，材料全部为公开样例或合成文本。8 次请求（313–4815 字节）均返回 HTTP 503，message=System One endpoint is disabled。

公开样例 trace_id：f3b0510a-7c61-4159-8e32-ba50cdc1f9e8、36ec1f71-23e4-4961-8cd2-cef5374d5b8e。

curl HTTP/1.1 与协商 HTTP/2 的对照也返回同一错误；实际协商版本均为 HTTP/1.1。Python 和 curl 的默认网络经过本机 HTTP 代理 127.0.0.1:7897。Python 禁用代理的直连失败为 Connection reset by peer，不能据此断定代理就是原因。

同 Key 的 GET /v1/models 返回 200，模型标识中未发现 jev。该结果不证明整个服务停用。

## 当前结论与待验证项

尚未稳定调用。失败可在脱离知微业务代码的官方最小请求中复现，不能归因于 LFU 或上下文超长。需对比用户已成功的请求：是否同一本机 Key、同端点、同模型名、同网络出口。没有把 TeamoRouter Key 发往 TypeSafe，也没有擅自更换账号、模型或代理配置。

## 本地修复

保留 24 KiB 请求窗口及 LFU。错误信息只报告当前路由返回值，不再断言 Jev 整体停用。补齐 429 和 529 的短暂退避重试。真实上下文重放被自动审批拒绝后未执行，只做合成请求测试。

## 官方 SDK 对照

在临时虚拟环境安装 PyPI `typesafe-sdk==0.7.1`，使用文档中的 `TypeSafeClient(api_key=..., base_url="https://api.teamorouter.com", model="jev")`，timeout=10 秒，保留默认重试。

英文官方样例与中文合成样例均抛出 `TypeSafeInternalServerError`，HTTP 503，message=`System One endpoint is disabled`。最终响应 trace_id 分别为 `49244511-552e-490a-bca2-24b41100a3a7` 和 `5cf39c14-9fb5-48a2-b415-ec99a5ccce2f`。耗时 2.96 / 2.39 秒，成功 0/2。没有因为更换 SDK 而恢复。

复现：`/tmp/yourself-typesafe-sdk/bin/python scripts/check_jev_sdk.py`。该脚本只读取配置用于原服务鉴权，发送公开/合成输入，不读取历史上下文。
