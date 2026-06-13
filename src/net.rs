#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TcpNoDelay {
    #[default]
    Unspecified,
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketTuning {
    pub tcp_no_delay: TcpNoDelay,
}

impl SocketTuning {
    pub fn with_tcp_no_delay(mut self, value: TcpNoDelay) -> Self {
        self.tcp_no_delay = value;
        self
    }
}
