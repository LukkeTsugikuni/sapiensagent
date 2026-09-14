use crate::providers::Message;
use anyhow::Result;
use async_trait::async_trait;
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub session_id: String,
    pub task: String,
    pub messages: Vec<Message>,
    pub cancelled: bool,
}
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(&self, context: &AgentContext) -> Result<String>;
}
pub struct AgentLoop<P> {
    pub provider: P,
}
impl<P: ModelProvider> AgentLoop<P> {
    pub async fn run(&self, context: &mut AgentContext) -> Result<String> {
        if context.cancelled {
            anyhow::bail!("task cancelled");
        }
        self.provider.complete(context).await
    }
}
