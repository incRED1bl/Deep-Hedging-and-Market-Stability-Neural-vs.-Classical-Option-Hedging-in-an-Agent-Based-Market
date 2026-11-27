use crate::application::ports::{ShockAddress, ShockSource};

#[derive(Clone, Copy, Debug, Default)]
pub struct SeededShocks;

impl ShockSource for SeededShocks {
    fn draw(&self, address: ShockAddress) -> u64 {
        let agent = address.agent.map_or(0, |id| u64::from(id.get()) + 1);
        let time_and_draw = (u64::from(address.step.get()) << 32) | u64::from(address.draw_index);
        let agent_and_kind = (agent << 8) | address.kind as u64;
        let market = mix(address.market_seed ^ 0x9e37_79b9_7f4a_7c15);
        mix(mix(market ^ time_and_draw) ^ agent_and_kind)
    }
}

fn mix(value: u64) -> u64 {
    let value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
