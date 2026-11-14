#[derive(Debug)]
pub struct Config {
    pub auth: Auth,
    pub max_tokens: u32,
    pub model: String,
}