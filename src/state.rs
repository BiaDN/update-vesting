use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, msg, pubkey::Pubkey};

pub const PROGRAM_VERSION: u64 = 2;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
#[repr(C)]
pub struct StreamInstruction {
    pub start_time: u64,
    pub end_time: u64,
    pub deposited_amount: u64,
    pub total_amount: u64,
    pub period: u64,
    pub cliff: u64,
    pub cliff_amount: u64,
    pub cancelable_by_sender: bool,
    pub cancelable_by_recipient: bool,
    pub withdrawal_public: bool,
    pub transferable_by_sender: bool,
    pub transferable_by_recipient: bool,
    pub release_rate: u64,
    pub stream_name: String,
}

impl Default for StreamInstruction {
    fn default() -> Self {
        StreamInstruction {
            start_time: 0,
            end_time: 0,
            deposited_amount: 0,
            total_amount: 0,
            period: 1,
            cliff: 0,
            cliff_amount: 0,
            cancelable_by_sender: true,
            cancelable_by_recipient: false,
            withdrawal_public: false,
            transferable_by_sender: false,
            transferable_by_recipient: true,
            release_rate: 0,
            stream_name: "Stream".to_string(),
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default, Debug)]
#[repr(C)]
pub struct TokenStreamData {
    pub magic: u64,
    pub created_at: u64,
    pub withdrawn_amount: u64,
    pub canceled_at: u64,
    pub closable_at: u64,
    pub last_withdrawn_at: u64,
    pub sender: Pubkey,
    pub sender_tokens: Pubkey,
    pub recipient: Pubkey,
    pub recipient_tokens: Pubkey,
    pub mint: Pubkey,
    pub escrow_tokens: Pubkey,
    pub ix: StreamInstruction,
}

#[allow(clippy::too_many_arguments)]
impl TokenStreamData {
    pub fn new(
        created_at: u64,
        sender: Pubkey,
        sender_tokens: Pubkey,
        recipient: Pubkey,
        recipient_tokens: Pubkey,
        mint: Pubkey,
        escrow_tokens: Pubkey,
        start_time: u64,
        end_time: u64,
        deposited_amount: u64,
        total_amount: u64,
        period: u64,
        cliff: u64,
        cliff_amount: u64,
        cancelable_by_sender: bool,
        cancelable_by_recipient: bool,
        withdrawal_public: bool,
        transferable_by_sender: bool,
        transferable_by_recipient: bool,
        release_rate: u64,
        stream_name: String,
    ) -> Self {
        let ix = StreamInstruction {
            start_time,
            end_time,
            deposited_amount,
            total_amount,
            period,
            cliff,
            cliff_amount,
            cancelable_by_sender,
            cancelable_by_recipient,
            withdrawal_public,
            transferable_by_sender,
            transferable_by_recipient,
            release_rate,
            stream_name,
        };

        Self {
            magic: PROGRAM_VERSION,
            created_at,
            withdrawn_amount: 0,
            canceled_at: 0,
            closable_at: end_time,
            last_withdrawn_at: 0,
            sender,
            sender_tokens,
            recipient,
            recipient_tokens,
            mint,
            escrow_tokens,
            ix,
        }
    }

    pub fn available(&self, now: u64) -> u64 {
        if self.ix.start_time > now || self.ix.cliff > now {
            return 0;
        }

        if now >= self.ix.end_time && self.ix.release_rate == 0 {
            return self.ix.deposited_amount - self.withdrawn_amount;
        }

        let cliff = if self.ix.cliff > 0 {
            self.ix.cliff
        } else {
            self.ix.start_time
        };

        let cliff_amount = if self.ix.cliff_amount > 0 {
            self.ix.cliff_amount
        } else {
            0
        };

        let num_periods = (self.ix.end_time - cliff) as f64 / self.ix.period as f64;
        let period_amount = if self.ix.release_rate > 0 {
            self.ix.release_rate as f64
        } else {
            (self.ix.total_amount - cliff_amount) as f64 / num_periods
        };
        let periods_passed = (now - cliff) / self.ix.period;
        (periods_passed as f64 * period_amount) as u64 + cliff_amount - self.withdrawn_amount
    }

    pub fn closable(&self) -> u64 {
        let cliff_time = if self.ix.cliff > 0 {
            self.ix.cliff
        } else {
            self.ix.start_time
        };

        let cliff_amount = if self.ix.cliff_amount > 0 {
            self.ix.cliff_amount
        } else {
            0
        };
        if self.ix.deposited_amount < cliff_amount {
            return cliff_time;
        }
        let seconds_nr = self.ix.end_time - cliff_time;

        let amount_per_second = if self.ix.release_rate > 0 {
            self.ix.release_rate / self.ix.period
        } else {
            ((self.ix.total_amount - cliff_amount) / seconds_nr) as u64
        };
        let seconds_left = ((self.ix.deposited_amount - cliff_amount) / amount_per_second) + 1;

        msg!(
            "Release {}, Period {}, seconds left {}",
            self.ix.release_rate,
            self.ix.period,
            seconds_left
        );
        if cliff_time + seconds_left > self.ix.end_time && self.ix.release_rate == 0 {
            self.ix.end_time
        } else {
            cliff_time + seconds_left
        }
    }
}

#[derive(Debug)]
pub struct InitializeAccounts<'a> {
    pub sender: AccountInfo<'a>,
    pub sender_tokens: AccountInfo<'a>,
    pub recipient: AccountInfo<'a>,
    pub recipient_tokens: AccountInfo<'a>,
    pub metadata: AccountInfo<'a>,
    pub escrow_tokens: AccountInfo<'a>,
    pub mint: AccountInfo<'a>,
    pub rent: AccountInfo<'a>,
    pub token_program: AccountInfo<'a>,
    pub associated_token_program: AccountInfo<'a>,
    pub system_program: AccountInfo<'a>,
}

pub struct WithdrawAccounts<'a> {
    pub withdraw_authority: AccountInfo<'a>,
    pub sender: AccountInfo<'a>,
    pub recipient: AccountInfo<'a>,
    pub recipient_tokens: AccountInfo<'a>,
    pub metadata: AccountInfo<'a>,
    pub escrow_tokens: AccountInfo<'a>,
    pub mint: AccountInfo<'a>,
    pub token_program: AccountInfo<'a>,
}

pub struct CancelAccounts<'a> {
    pub cancel_authority: AccountInfo<'a>,
    pub sender: AccountInfo<'a>,
    pub sender_tokens: AccountInfo<'a>,
    pub recipient: AccountInfo<'a>,
    pub recipient_tokens: AccountInfo<'a>,
    pub metadata: AccountInfo<'a>,
    pub escrow_tokens: AccountInfo<'a>,
    pub mint: AccountInfo<'a>,
    pub token_program: AccountInfo<'a>,
}

pub struct TransferAccounts<'a> {
    pub authorized_wallet: AccountInfo<'a>,
    pub new_recipient: AccountInfo<'a>,
    pub new_recipient_tokens: AccountInfo<'a>,
    pub metadata: AccountInfo<'a>,
    pub escrow_tokens: AccountInfo<'a>,
    pub mint: AccountInfo<'a>,
    pub rent: AccountInfo<'a>,
    pub token_program: AccountInfo<'a>,
    pub associated_token_program: AccountInfo<'a>,
    pub system_program: AccountInfo<'a>,
}

#[derive(Debug)]
pub struct TopUpAccounts<'a> {
    pub sender: AccountInfo<'a>,
    pub sender_tokens: AccountInfo<'a>,
    pub metadata: AccountInfo<'a>,
    pub escrow_tokens: AccountInfo<'a>,
    pub mint: AccountInfo<'a>,
    pub token_program: AccountInfo<'a>,
}