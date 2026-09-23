# 一手来源与证据边界

核实日期：2026-09-22。以下链接支持产品事实和既有研究，不是对本文自创架构/权重的背书。

- **S1** TypeSafe, Introducing System One Models & Jev (2026-09-15). https://typesafe.ai/blog/introducing-system-one-models-and-jev
  官方披露 architecture/parallel sampler/RLCD；性能属于厂商特定评测。其 workflow 参考答案为其他模型概率，不是现场用户行为真值。
- **S2** TypeSafe Models. https://docs.typesafe.ai/models
  版本、文本输入、价格、上下文限制、语言差异、客户不可 fine-tune/LoRA、数据处理说明。
- **S3** TypeSafe API reference. https://docs.typesafe.ai/api
  POST /v1/systemone，Noul/Choice/Score 以及返回字段。question ID 不参与模型语义。
- **S4** TypeSafe Confidence. https://docs.typesafe.ai/confidence
  Confidence 由已有概率分布计算，不是独立准确率保证；Noul 没有此字段。
- **S5** TypeSafe AI primer. https://docs.typesafe.ai/introduction/machine-learning-primer
  RLCD 全称 Reinforcement Learning for Calibrated Decisions；未提供足以复现的奖励方程。
- **S6** TypeSafe Jev 1.13 jaggedness, reviewed 2026-09-17. https://docs.typesafe.ai/model-jaggedness/jev-1.13
  数值/时间、无关长上下文、注入、题目间不一致和非生成任务边界。
- **S7** Devlin et al., BERT (NAACL 2019). https://aclanthology.org/N19-1423/
  双向语言表示与任务微调，作为旧分类模型对照。
- **S8** Yin et al., Benchmarking Zero-shot Text Classification (EMNLP 2019). https://aclanthology.org/D19-1404/
  基于 entailment 的动态语义标签分类，零样本分类并非 Jev 首创。
- **S9** Guo et al., On Calibration of Modern Neural Networks (ICML 2017). https://proceedings.mlr.press/v70/guo17a.html
  现代神经网络校准问题及温度缩放。
- **S10** Park et al., Generative Agents (2023). https://arxiv.org/abs/2304.03442
  观察、记忆、反思和计划的公开研究基础，不证明主观意识。
- **S11** Anthropic, Effective harnesses for long-running agents (2025-11-26). https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
  跨会话进度、产物和测试的重要性。
- **S12** SQLite, Write-Ahead Logging. https://sqlite.org/wal.html
  WAL 并发和 checkpoint 边界。
- **S13** Tauri, Capabilities. https://v2.tauri.app/security/capabilities/
  前端能力控制和不受其保护的后端风险。
- **S14** ColaOS official home / about / blog. https://colaos.ai/zh/ ; https://colaos.ai/about ; https://colaos.ai/blog/
  官方产品方向：电脑中的持续伙伴、记忆、成长、awareness/memory/care，以及 self-reference 博客介绍。部分页面动态加载，仅取得官方索引摘要，未据此声称掌握其内部实现。
- **S15** TypeSafe Introduction. https://docs.typesafe.ai/introduction
  多问题并行独立评估、类型化原语和原子问题设计。

本文的情绪方程、主动对话效用函数、MindRuntime、PersonaPack 和测试用阈值都是架构建议，不是 Jev / ColaOS 公布的内部算法。
