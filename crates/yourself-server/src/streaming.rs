//! Incremental SSE decoding. Reasoning/tool-only frames never become visible placeholders.
use crate::data::AppResult;
use serde_json::{json, Value};
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    pub text: String,
    usage: Value,
    finish: Option<String>,
    done: bool,
}
impl Decoder {
    pub fn feed(&mut self, chunk: &[u8]) -> AppResult<()> {
        self.buffer.extend_from_slice(chunk);
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)
                .map_err(|_| "流式响应编码错误")?
                .trim();
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    self.done = true;
                    continue;
                }
                if data.is_empty() {
                    continue;
                }
                let v: Value = serde_json::from_str(data).map_err(|_| "流式响应格式错误")?;
                if v.get("error").is_some() {
                    return Err("模型流式响应返回错误".into());
                }
                if let Some(text) = v
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                {
                    self.text.push_str(text);
                }
                if let Some(reason) = v
                    .pointer("/choices/0/finish_reason")
                    .and_then(Value::as_str)
                {
                    self.finish = Some(reason.into());
                }
                if v["usage"].is_object() {
                    self.usage = v["usage"].clone();
                }
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> AppResult<Value> {
        if !self.buffer.is_empty() {
            self.feed(b"\n")?;
        }
        if !self.done && self.finish.is_none() {
            return Err("模型流式响应中断，未收到结束标记。".into());
        }
        if self.text.trim().is_empty() {
            return Err("模型返回了空内容，请重试。".into());
        }
        Ok(
            json!({"choices":[{"message":{"role":"assistant","content":self.text},"finish_reason":self.finish.unwrap_or("stop".into())}],"usage":self.usage}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_unicode_and_empty_reasoning_frames() {
        let input="data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hidden\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let mut d = Decoder::default();
        for b in input.as_bytes() {
            d.feed(&[*b]).unwrap();
        }
        assert_eq!(
            d.finish().unwrap()["choices"][0]["message"]["content"],
            "你好"
        );
        assert!(Decoder::default().finish().is_err());
    }
}
