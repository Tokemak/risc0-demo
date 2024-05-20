use core::fmt;

use alloy_primitives::{address, utils::format_units, Address, U256};
use alloy_sol_types::sol;
use risc0_steel::BlockCommitment;

// Curve/Convex cbETH/ETH pool
pub const CURVE_POOL_ADDRESS: Address = address!("06325440D014E39736583C165C2963BA99FAF14E");
pub const CURVE_LP_ADDRESS: Address = address!("5b6C539b224014A09B3388e51CaAA8e354c959C8");
pub const CBETH_ADDRESS: Address = address!("Be9895146f7AF43049ca1c1AE358B0541Ea49704");
pub const CBETH_CHAINLINK_ORACLE: Address = address!("F017fcB346A1885194689bA23Eff2fE6fA5C483b");

pub const DAY_IN_SECONDS: u64 = 24 * 60 * 60;
pub const BLOCK_GRANULARITY: u64 = DAY_IN_SECONDS / 12;
pub const BLOCKS_TO_QUERY: u64 = (3 * DAY_IN_SECONDS) / 12;

sol! {
    interface CurvePoolInterface {
        function get_virtual_price() public view returns (uint256);
        function coins(uint256) public view returns (address);
        function balances(uint256) public view returns (uint256);
    }

    interface ERC20Interface {
        function totalSupply() public view returns (uint256);
        function decimals() public view returns (uint8);
    }

    interface ChainlinkInterface {
        function latestRoundData() public view returns (uint80,int256,uint256,uint256,uint80);
    }

    interface cbETHInterface {
        function exchangeRate() public view returns (uint256);
    }
}

sol! {
    #[derive(Debug)]
    struct LstDexStats {
        BlockCommitment commitment;
        uint256 baseYield;
    }
}

impl fmt::Display for LstDexStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base_yield = u256_to_f64(self.baseYield, 18) * 100.0;
        write!(
            f,
            "LstDexStats: baseYield={:.2}% (blockNumber={}, blockHash={})",
            base_yield, self.commitment.blockNumber, self.commitment.blockHash
        )
    }
}

#[derive(Debug)]
pub struct DexStatsInput {
    pub timestamp: u64,
    pub block_number: u64,
    pub lst_backing: U256,
}

#[derive(Debug)]
pub struct DexStatsOutput {
    pub base_yield: f64,
}

pub fn calculate_dex_stats(input: &[DexStatsInput], skip: usize) -> DexStatsOutput {
    // unchecked: verify that the provided history is as long as it can be
    // we want X days of data, but that may not exist. If it doesn't exist we need to check contract creation
    // assumptions: provided data has already been verified in the guest program

    assert!(input.len() > 0, "input data not long enough");

    for (index, item) in input[1..].iter().enumerate() {
        let prior_block_number = input[index].block_number;

        // verify that the list is sorted
        assert!(item.block_number > prior_block_number, "list not sorted");

        // verify that the list is approximately daily
        // note: we may need to do this differently to account for chain pausing
        let block_delta = item.block_number - prior_block_number;
        assert!(block_delta == BLOCK_GRANULARITY, "provided data not at correct granularity")
    }

    // get every skip item to calculate rolling change
    let mut resampled = Vec::new();
    for (index, item) in input.iter().rev().enumerate() {
        if index % skip == 0 {
            resampled.push(item);
        }
    }
    resampled.reverse();

    assert!(resampled.len() > 0, "resampled data insufficient");

    // TODO: switch to an ema
    let mut total = 0.0;
    let mut prior_backing = u256_to_f64(resampled.first().unwrap().lst_backing, 18);
    let mut prior_timestamp = resampled.first().unwrap().timestamp;
    for item in resampled[1..].iter() {
        let time_delta_seconds = item.timestamp - prior_timestamp;
        let annualizer = ((DAY_IN_SECONDS * 365) / time_delta_seconds) as f64;

        let current = u256_to_f64(item.lst_backing, 18);
        let change = (current / prior_backing - 1.0) * annualizer;
        total += change;
        prior_backing = current;
        prior_timestamp = item.timestamp;
    }

    let base_yield = total / (resampled.len() - 1) as f64;

    DexStatsOutput { base_yield }
}

fn u256_to_f64(value: U256, units: u8) -> f64 {
    let str_fmt = format_units(value, units).unwrap();

    str_fmt.parse::<f64>().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_should_work() {
        let b = U256::from_str_radix("100", 10).unwrap();
        println!("b={:?}", b);
    }

    #[test]
    fn it_should_calculate_backing_avg() {
        let inputs = build_input(1716129570, &vec![100.0, 100.01, 100.10, 100.15, 100.25]);

        let expected = 0.2279345389;

        let res = calculate_dex_stats(&inputs, 1);
        let delta = (res.base_yield - expected).abs();
        assert!(delta <= 0.00000001);
        println!("{:?}", res);
    }

    fn build_input(start_timestamp: u64, input_values: &[f64]) -> Vec<DexStatsInput> {
        input_values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let lst_backing =
                    U256::from_str_radix(&((v * 10_f64.powf(18.0)).to_string()), 10).unwrap();
                let timestamp = start_timestamp + (i as u64 * DAY_IN_SECONDS); // Increment timestamp by 1 day for each input value
                DexStatsInput {
                    timestamp,
                    block_number: (i as u64 * BLOCK_GRANULARITY),
                    lst_backing,
                }
            })
            .collect()
    }
}
