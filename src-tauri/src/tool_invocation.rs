use crate::error::GlossError;
use crate::redaction::redact_text_paths;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct ToolInvocationReceiptV1 {
    pub schema: String,
    pub receipt_id: String,
    pub tool: String,
    pub action: String,
    pub args_redacted: Vec<String>,
    pub timeout_ms: u64,
    pub elapsed_ms: u128,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub timed_out: bool,
    pub stderr_sha256: Option<String>,
    pub stderr_len: usize,
    pub stderr_preview: Option<String>,
    pub stdout_sha256: Option<String>,
    pub stdout_len: usize,
}

#[derive(Debug)]
pub struct ToolInvocationOutput {
    pub receipt: ToolInvocationReceiptV1,
    pub stdout: Vec<u8>,
}

struct ReceiptBuildInput<'a> {
    tool: &'a str,
    action: &'a str,
    args_redacted: Vec<String>,
    timeout_ms: u64,
    elapsed_ms: u128,
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
    stderr: &'a [u8],
    stdout: &'a [u8],
}

pub async fn run_tool_output_receipt(
    tool: &str,
    action: &str,
    args: &[String],
    args_redacted: Vec<String>,
    timeout: Duration,
) -> Result<ToolInvocationOutput, GlossError> {
    let started = Instant::now();
    let mut command = tokio::process::Command::new(tool);
    command
        .args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let timeout_ms = timeout.as_millis() as u64;
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => {
            let receipt = build_receipt(ReceiptBuildInput {
                tool,
                action,
                args_redacted,
                timeout_ms,
                elapsed_ms: started.elapsed().as_millis(),
                exit_code: output.status.code(),
                success: output.status.success(),
                timed_out: false,
                stderr: &output.stderr,
                stdout: &output.stdout,
            });
            Ok(ToolInvocationOutput {
                receipt,
                stdout: output.stdout,
            })
        }
        Ok(Err(err)) => Ok(ToolInvocationOutput {
            receipt: build_receipt(ReceiptBuildInput {
                tool,
                action,
                args_redacted,
                timeout_ms,
                elapsed_ms: started.elapsed().as_millis(),
                exit_code: None,
                success: false,
                timed_out: false,
                stderr: err.to_string().as_bytes(),
                stdout: &[],
            }),
            stdout: Vec::new(),
        }),
        Err(_) => Ok(ToolInvocationOutput {
            receipt: build_receipt(ReceiptBuildInput {
                tool,
                action,
                args_redacted,
                timeout_ms,
                elapsed_ms: started.elapsed().as_millis(),
                exit_code: None,
                success: false,
                timed_out: true,
                stderr: &[],
                stdout: &[],
            }),
            stdout: Vec::new(),
        }),
    }
}

pub async fn run_tool_status_receipt(
    tool: &str,
    action: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<ToolInvocationReceiptV1, GlossError> {
    let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let output = run_tool_output_receipt(tool, action, &args, args.clone(), timeout).await?;
    Ok(output.receipt)
}

fn build_receipt(input: ReceiptBuildInput<'_>) -> ToolInvocationReceiptV1 {
    ToolInvocationReceiptV1 {
        schema: "ToolInvocationReceiptV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        tool: input.tool.to_string(),
        action: input.action.to_string(),
        args_redacted: input.args_redacted,
        timeout_ms: input.timeout_ms,
        elapsed_ms: input.elapsed_ms,
        exit_code: input.exit_code,
        success: input.success,
        timed_out: input.timed_out,
        stderr_sha256: digest(input.stderr),
        stderr_len: input.stderr.len(),
        stderr_preview: sanitized_preview(input.stderr),
        stdout_sha256: digest(input.stdout),
        stdout_len: input.stdout.len(),
    }
}

fn digest(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Some(format!("{:x}", hasher.finalize()))
}

fn sanitized_preview(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let text = redact_text_paths(&String::from_utf8_lossy(bytes).replace(['\n', '\r', '\t'], " "))
        .split_whitespace()
        .map(redact_stderr_token)
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        None
    } else {
        Some(text.chars().take(240).collect())
    }
}

fn redact_stderr_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if token.contains('/')
        || token.contains('\\')
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("authorization")
        || lower.contains("api_key")
        || token.starts_with("sk-")
    {
        "[redacted]".to_string()
    } else {
        token.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_preview_redacts_paths_and_secrets() {
        let preview =
            sanitized_preview(b"error /tmp/private/video.mp4 Authorization: Bearer sk-test")
                .unwrap();
        assert!(preview.contains("[redacted]"));
        assert!(!preview.contains("/tmp/private"));
        assert!(!preview.contains("sk-test"));
    }
}
