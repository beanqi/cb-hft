use crate::config::AppConfig;
use crate::event::ExchangeEvent;
use crate::order::StrategyCommand;
use crate::ring::{
    CommandConsumer, CommandProducer, CommandRingPair, EventConsumer, EventProducer, EventRingPair,
    RingError,
};

pub struct AppTopology {
    command_rings: Vec<(CommandProducer, CommandConsumer)>,
    order_event_rings: Vec<(EventProducer, EventConsumer)>,
    account_event_rings: Vec<(EventProducer, EventConsumer)>,
}

impl AppTopology {
    pub fn from_config(config: &AppConfig) -> Result<Self, RingError> {
        let symbol_count = config.products.len();
        let mut command_rings = Vec::with_capacity(symbol_count);
        let mut order_event_rings = Vec::with_capacity(symbol_count);
        let mut account_event_rings = Vec::with_capacity(symbol_count);

        for _ in 0..symbol_count {
            command_rings.push(CommandRingPair::new(config.ring.cmd_capacity)?);
            order_event_rings.push(EventRingPair::new(config.ring.order_event_capacity)?);
            account_event_rings.push(EventRingPair::new(config.ring.account_event_capacity)?);
        }

        Ok(Self {
            command_rings,
            order_event_rings,
            account_event_rings,
        })
    }

    pub fn symbol_count(&self) -> usize {
        self.command_rings.len()
    }

    pub fn command_rings(&self) -> &[(CommandProducer, CommandConsumer)] {
        &self.command_rings
    }

    pub fn order_event_rings(&self) -> &[(EventProducer, EventConsumer)] {
        &self.order_event_rings
    }

    pub fn account_event_rings(&self) -> &[(EventProducer, EventConsumer)] {
        &self.account_event_rings
    }
}

#[allow(dead_code)]
type _TopologyCommand = StrategyCommand;
#[allow(dead_code)]
type _TopologyEvent = ExchangeEvent;
