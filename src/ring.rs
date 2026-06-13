use std::collections::VecDeque;

use crate::event::ExchangeEvent;
use crate::order::StrategyCommand;
use crate::types::SymbolId;

pub struct CommandRingPair;

pub struct EventRingPair;

pub struct CommandProducer {
    inner: rtrb::Producer<StrategyCommand>,
}

pub struct CommandConsumer {
    inner: rtrb::Consumer<StrategyCommand>,
}

pub struct EventProducer {
    inner: rtrb::Producer<ExchangeEvent>,
}

pub struct EventConsumer {
    inner: rtrb::Consumer<ExchangeEvent>,
}

impl CommandRingPair {
    pub fn new(capacity: usize) -> Result<(CommandProducer, CommandConsumer), RingError> {
        if capacity == 0 {
            return Err(RingError::ZeroCapacity);
        }
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        Ok((
            CommandProducer { inner: producer },
            CommandConsumer { inner: consumer },
        ))
    }
}

impl CommandProducer {
    pub fn push(&mut self, command: StrategyCommand) -> Result<(), RingError> {
        self.inner.push(command).map_err(|_| RingError::Full)
    }
}

impl CommandConsumer {
    pub fn pop(&mut self) -> Option<StrategyCommand> {
        self.inner.pop().ok()
    }
}

impl EventRingPair {
    pub fn new(capacity: usize) -> Result<(EventProducer, EventConsumer), RingError> {
        if capacity == 0 {
            return Err(RingError::ZeroCapacity);
        }
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        Ok((
            EventProducer { inner: producer },
            EventConsumer { inner: consumer },
        ))
    }
}

impl EventProducer {
    pub fn push(&mut self, event: ExchangeEvent) -> Result<(), RingError> {
        self.inner.push(event).map_err(|_| RingError::Full)
    }
}

impl EventConsumer {
    pub fn pop(&mut self) -> Option<ExchangeEvent> {
        self.inner.pop().ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingError {
    InvalidSymbol,
    ZeroCapacity,
    Full,
}

#[derive(Debug)]
pub struct CommandRing {
    queue: VecDeque<StrategyCommand>,
    capacity: usize,
}

#[derive(Debug)]
pub struct CommandRings {
    rings: Vec<CommandRing>,
}

impl CommandRings {
    pub fn new(symbol_count: usize, capacity: usize) -> Result<Self, RingError> {
        if capacity == 0 {
            return Err(RingError::ZeroCapacity);
        }
        let rings = (0..symbol_count)
            .map(|_| CommandRing {
                queue: VecDeque::with_capacity(capacity),
                capacity,
            })
            .collect();
        Ok(Self { rings })
    }

    pub fn producer_mut(
        &mut self,
        symbol_id: SymbolId,
    ) -> Result<CommandRingProducer<'_>, RingError> {
        let ring = self
            .rings
            .get_mut(symbol_id.0 as usize)
            .ok_or(RingError::InvalidSymbol)?;
        Ok(CommandRingProducer { ring })
    }

    pub fn consumer_mut(
        &mut self,
        symbol_id: SymbolId,
    ) -> Result<CommandRingConsumer<'_>, RingError> {
        let ring = self
            .rings
            .get_mut(symbol_id.0 as usize)
            .ok_or(RingError::InvalidSymbol)?;
        Ok(CommandRingConsumer { ring })
    }
}

pub struct CommandRingProducer<'a> {
    ring: &'a mut CommandRing,
}

pub struct CommandRingConsumer<'a> {
    ring: &'a mut CommandRing,
}

impl CommandRingProducer<'_> {
    pub fn push(&mut self, command: StrategyCommand) -> Result<(), RingError> {
        if self.ring.queue.len() >= self.ring.capacity {
            return Err(RingError::Full);
        }
        self.ring.queue.push_back(command);
        Ok(())
    }
}

impl CommandRingConsumer<'_> {
    pub fn pop(&mut self) -> Option<StrategyCommand> {
        self.ring.queue.pop_front()
    }
}
