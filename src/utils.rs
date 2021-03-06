use std::iter::FromIterator;

use solana_program::{account_info::AccountInfo, program_error::ProgramError, program_pack::Pack};

pub fn duration_sanity(now: u64, start: u64, end: u64, cliff: u64) -> bool {
    let cliff_cond = if cliff == 0 {
        true
    } else {
        start <= cliff && cliff <= end
    };

    now < start && start < end && cliff_cond
}

pub fn unpack_token_account(
    account_info: &AccountInfo,
) -> Result<spl_token::state::Account, ProgramError> {
    if account_info.owner != &spl_token::id() {
        return Err(ProgramError::InvalidAccountData);
    }

    spl_token::state::Account::unpack(&account_info.data.borrow())
}

pub fn unpack_mint_account(
    account_info: &AccountInfo,
) -> Result<spl_token::state::Mint, ProgramError> {
    spl_token::state::Mint::unpack(&account_info.data.borrow())
}

pub fn pretty_time(t: u64) -> String {
    let seconds = t % 60;
    let minutes = (t / 60) % 60;
    let hours = (t / (60 * 60)) % 24;
    let days = t / (60 * 60 * 24);

    format!(
        "{} days, {} hours, {} minutes, {} seconds",
        days, hours, minutes, seconds
    )
}

pub fn encode_base10(amount: u64, decimal_places: usize) -> String {
    let mut s: Vec<char> = format!("{:0width$}", amount, width = 1 + decimal_places)
        .chars()
        .collect();
    s.insert(s.len() - decimal_places, '.');

    String::from_iter(&s)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}



