use borsh::BorshSerialize;
use solana_program::{
    borsh as solana_borsh,
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    system_instruction, system_program, sysvar,
    sysvar::{clock::Clock, rent::Rent, Sysvar},
    // sysvar::{clock::Clock, rent::Rent, Sysvar},

};
use spl_associated_token_account::{instruction:: create_associated_token_account, get_associated_token_address};

use crate::error::StreamFlowError::{
    AccountsNotWritable, InvalidMetadata, MintMismatch, StreamClosed, TransferNotAllowed,
};
use crate::state::{
    CancelAccounts, InitializeAccounts, StreamInstruction, TokenStreamData, TopUpAccounts,
    TransferAccounts, WithdrawAccounts,
};
use crate::utils::{
    duration_sanity, encode_base10, pretty_time, unpack_mint_account, unpack_token_account,
};

const MAX_STRING_SIZE: usize = 200;

pub fn create(
    program_id: &Pubkey,
    acc: InitializeAccounts,
    ix: StreamInstruction,
) -> ProgramResult {
    msg!("Initializing SPL token stream");

    if !acc.escrow_tokens.data_is_empty() || !acc.metadata.data_is_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    if !acc.sender.is_writable
        || !acc.sender_tokens.is_writable
        || !acc.recipient.is_writable
        || !acc.recipient_tokens.is_writable
        || !acc.metadata.is_writable
        || !acc.escrow_tokens.is_writable
    {
        return Err(AccountsNotWritable.into());
    }

    let (escrow_tokens_pubkey, nonce) =
        Pubkey::find_program_address(&[acc.metadata.key.as_ref()], program_id);
    let recipient_tokens_key = get_associated_token_address(acc.recipient.key, acc.mint.key);

    if acc.system_program.key != &system_program::id()
        || acc.token_program.key != &spl_token::id()
        || acc.rent.key != &sysvar::rent::id()
        || acc.escrow_tokens.key != &escrow_tokens_pubkey
        || acc.recipient_tokens.key != &recipient_tokens_key
    {
        return Err(ProgramError::InvalidAccountData);
    }

    if !acc.sender.is_signer || !acc.metadata.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let sender_token_info = unpack_token_account(&acc.sender_tokens)?;
    let mint_info = unpack_mint_account(&acc.mint)?;

    if &sender_token_info.mint != acc.mint.key {
        return Err(MintMismatch.into());
    }

    let now = Clock::get()?.unix_timestamp as u64;
    if !duration_sanity(now, ix.start_time, ix.end_time, ix.cliff) {
        msg!("Error: Given timestamps are invalid");
        return Err(ProgramError::InvalidArgument);
    }

    if ix.stream_name.len() > MAX_STRING_SIZE {
        msg!("Error: Stream name too long!");
        return Err(ProgramError::InvalidArgument);
    }

    let mut metadata = TokenStreamData::new(
        now,
        *acc.sender.key,
        *acc.sender_tokens.key,
        *acc.recipient.key,
        *acc.recipient_tokens.key,
        *acc.mint.key,
        *acc.escrow_tokens.key,
        ix.start_time,
        ix.end_time,
        ix.deposited_amount,
        ix.total_amount,
        ix.period,
        ix.cliff,
        ix.cliff_amount,
        ix.cancelable_by_sender,
        ix.cancelable_by_recipient,
        ix.withdrawal_public,
        ix.transferable_by_sender,
        ix.transferable_by_recipient,
        ix.release_rate,
        ix.stream_name,
    );

    if ix.deposited_amount < ix.total_amount || ix.release_rate > 0 {
        metadata.closable_at = metadata.closable();
        msg!("Closable at: {}", metadata.closable_at);
    }

    let metadata_bytes = metadata.try_to_vec()?;
    let mut metadata_struct_size = metadata_bytes.len();
    while metadata_struct_size % 8 > 0 {
        metadata_struct_size += 1;
    }
    let tokens_struct_size = spl_token::state::Account::LEN;

    let cluster_rent = Rent::get()?;
    let metadata_rent = cluster_rent.minimum_balance(metadata_struct_size);
    let mut tokens_rent = cluster_rent.minimum_balance(tokens_struct_size);
    if acc.recipient_tokens.data_is_empty() {
        tokens_rent += cluster_rent.minimum_balance(tokens_struct_size);
    }


    if acc.sender.lamports() < metadata_rent + tokens_rent {
        msg!("Error: Insufficient funds in {}", acc.sender.key);
        return Err(ProgramError::InsufficientFunds);
    }

    if sender_token_info.amount < ix.deposited_amount {
        msg!("Error: Insufficient tokens in sender's wallet");
        return Err(ProgramError::InsufficientFunds);
    }

    if acc.recipient_tokens.data_is_empty() {
        msg!("Initializing recipient's associated token account");
        invoke(
            &create_associated_token_account(acc.sender.key, acc.recipient.key, acc.mint.key),
            &[
                acc.sender.clone(),
                acc.recipient_tokens.clone(),
                acc.recipient.clone(),
                acc.mint.clone(),
                acc.system_program.clone(),
                acc.token_program.clone(),
                acc.rent.clone(),
            ],
        )?;
    }

    msg!("Creating account for holding metadata");
    invoke(
        &system_instruction::create_account(
            acc.sender.key,
            acc.metadata.key,
            metadata_rent,
            metadata_struct_size as u64,
            program_id,
        ),
        &[
            acc.sender.clone(),
            acc.metadata.clone(),
            acc.system_program.clone(),
        ],
    )?;

    let mut data = acc.metadata.try_borrow_mut_data()?;
    data[0..metadata_bytes.len()].clone_from_slice(&metadata_bytes);

    let seeds = [acc.metadata.key.as_ref(), &[nonce]];
    msg!("Creating account for holding tokens");
    invoke_signed(
        &system_instruction::create_account(
            acc.sender.key,
            acc.escrow_tokens.key,
            cluster_rent.minimum_balance(tokens_struct_size),
            tokens_struct_size as u64,
            &spl_token::id(),
        ),
        &[
            acc.sender.clone(),
            acc.escrow_tokens.clone(),
            acc.system_program.clone(),
        ],
        &[&seeds],
    )?;

    msg!("Initializing escrow account for {} token", acc.mint.key);
    invoke(
        &spl_token::instruction::initialize_account(
            acc.token_program.key,
            acc.escrow_tokens.key,
            acc.mint.key,
            acc.escrow_tokens.key,
        )?,
        &[
            acc.token_program.clone(),
            acc.escrow_tokens.clone(),
            acc.mint.clone(),
            acc.escrow_tokens.clone(),
            acc.rent.clone(),
        ],
    )?;

    msg!("Moving funds into escrow account");
    invoke(
        &spl_token::instruction::transfer(
            acc.token_program.key,
            acc.sender_tokens.key,
            acc.escrow_tokens.key,
            acc.sender.key,
            &[],
            metadata.ix.deposited_amount,
        )?,
        &[
            acc.sender_tokens.clone(),
            acc.escrow_tokens.clone(),
            acc.sender.clone(),
            acc.token_program.clone(),
        ],
    )?;

    msg!(
        "Successfully initialized {} {} token stream for {}",
        encode_base10(metadata.ix.deposited_amount, mint_info.decimals.into()),
        metadata.mint,
        acc.recipient.key
    );
    msg!("Called by {}", acc.sender.key);
    msg!("Metadata written in {}", acc.metadata.key);
    msg!("Funds locked in {}", acc.escrow_tokens.key);
    msg!(
        "Stream duration is {}",
        pretty_time(metadata.ix.end_time - metadata.ix.start_time)
    );

    if metadata.ix.cliff > 0 && metadata.ix.cliff_amount > 0 {
        msg!("Cliff happens at {}", pretty_time(metadata.ix.cliff));
    }

    return Ok(());
}

pub fn withdraw(program_id: &Pubkey, acc: WithdrawAccounts, amount: u64) -> ProgramResult {
    msg!("Withdrawing from SPL token stream");

    if acc.escrow_tokens.data_is_empty()
        || acc.escrow_tokens.owner != &spl_token::id()
        || acc.metadata.data_is_empty()
        || acc.metadata.owner != program_id
    {
        return Err(ProgramError::UninitializedAccount);
    }

    if !acc.recipient.is_writable
        || !acc.recipient_tokens.is_writable
        || !acc.metadata.is_writable
        || !acc.escrow_tokens.is_writable
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let (escrow_tokens_pubkey, nonce) =
        Pubkey::find_program_address(&[acc.metadata.key.as_ref()], program_id);
    let recipient_tokens_key = get_associated_token_address(acc.recipient.key, acc.mint.key);

    if acc.token_program.key != &spl_token::id()
        || acc.escrow_tokens.key != &escrow_tokens_pubkey
        || acc.recipient_tokens.key != &recipient_tokens_key
        || acc.withdraw_authority.key != acc.recipient.key
    {
        return Err(ProgramError::InvalidAccountData);
    }

    if !acc.withdraw_authority.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let mut data = acc.metadata.try_borrow_mut_data()?;
    let mut metadata: TokenStreamData = match solana_borsh::try_from_slice_unchecked(&data) {
        Ok(v) => v,
        Err(_) => return Err(InvalidMetadata.into()),
    };

    let mint_info = unpack_mint_account(&acc.mint)?;

    if acc.recipient.key != &metadata.recipient
        || acc.recipient_tokens.key != &metadata.recipient_tokens
        || acc.mint.key != &metadata.mint
        || acc.escrow_tokens.key != &metadata.escrow_tokens
    {
        msg!("Error: Metadata does not match given accounts");
        return Err(ProgramError::InvalidAccountData);
    }

    let now = Clock::get()?.unix_timestamp as u64;
    let available = metadata.available(now);
    let requested: u64;

    if amount > available {
        msg!("Amount requested for withdraw is more than what is available");
        return Err(ProgramError::InvalidArgument);
    }

    if amount == 0 {
        requested = available;
    } else {
        requested = amount;
    }

    let seeds = [acc.metadata.key.as_ref(), &[nonce]];
    invoke_signed(
        &spl_token::instruction::transfer(
            acc.token_program.key,
            acc.escrow_tokens.key,
            acc.recipient_tokens.key,
            acc.escrow_tokens.key,
            &[],
            requested,
        )?,
        &[
            acc.escrow_tokens.clone(),
            acc.recipient_tokens.clone(),
            acc.escrow_tokens.clone(),
            acc.token_program.clone(),
        ],
        &[&seeds],
    )?;

    metadata.withdrawn_amount += requested;
    metadata.last_withdrawn_at = now;
    let bytes = metadata.try_to_vec()?;
    data[0..bytes.len()].clone_from_slice(&bytes);

    if metadata.withdrawn_amount == metadata.ix.deposited_amount {
        if !acc.sender.is_writable || acc.sender.key != &metadata.sender {
            return Err(ProgramError::InvalidAccountData);
        }

        let escrow_tokens_rent = acc.escrow_tokens.lamports();
        msg!(
            "Returning {} lamports (rent) to {}",
            escrow_tokens_rent,
            acc.sender.key
        );

        invoke_signed(
            &spl_token::instruction::close_account(
                acc.token_program.key,
                acc.escrow_tokens.key,
                acc.sender.key,
                acc.escrow_tokens.key,
                &[],
            )?,
            &[
                acc.escrow_tokens.clone(),
                acc.sender.clone(),
                acc.escrow_tokens.clone(),
            ],
            &[&seeds],
        )?;
    }

    msg!(
        "Withdrawn: {} {} tokens",
        encode_base10(requested, mint_info.decimals.into()),
        metadata.mint
    );
    msg!(
        "Remaining: {} {} tokens",
        encode_base10(
            metadata.ix.deposited_amount - metadata.withdrawn_amount,
            mint_info.decimals.into()
        ),
        metadata.mint
    );

    Ok(())
}

pub fn cancel(program_id: &Pubkey, acc: CancelAccounts) -> ProgramResult {
    msg!("Cancelling SPL token stream");

    if acc.escrow_tokens.data_is_empty()
        || acc.escrow_tokens.owner != &spl_token::id()
        || acc.metadata.data_is_empty()
        || acc.metadata.owner != program_id
    {
        return Err(ProgramError::UninitializedAccount);
    }

    if !acc.sender.is_writable
        || !acc.sender_tokens.is_writable
        || !acc.recipient.is_writable
        || !acc.recipient_tokens.is_writable
        || !acc.metadata.is_writable
        || !acc.escrow_tokens.is_writable
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let (escrow_tokens_pubkey, nonce) =
        Pubkey::find_program_address(&[acc.metadata.key.as_ref()], program_id);
    let recipient_tokens_key = get_associated_token_address(acc.recipient.key, acc.mint.key);

    if acc.token_program.key != &spl_token::id()
        || acc.escrow_tokens.key != &escrow_tokens_pubkey
        || acc.recipient_tokens.key != &recipient_tokens_key
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut data = acc.metadata.try_borrow_mut_data()?;
    let mut metadata: TokenStreamData = match solana_borsh::try_from_slice_unchecked(&data) {
        Ok(v) => v,
        Err(_) => return Err(InvalidMetadata.into()),
    };
    let mint_info = unpack_mint_account(&acc.mint)?;

    let now = Clock::get()?.unix_timestamp as u64;
    msg!("Now: {}, closable at {}", now, metadata.closable_at);
    if now < metadata.closable_at {
        if acc.cancel_authority.key != acc.sender.key {
            return Err(ProgramError::InvalidAccountData);
        }
        if !acc.cancel_authority.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }
    }

    if acc.sender.key != &metadata.sender
        || acc.sender_tokens.key != &metadata.sender_tokens
        || acc.recipient.key != &metadata.recipient
        || acc.recipient_tokens.key != &metadata.recipient_tokens
        || acc.mint.key != &metadata.mint
        || acc.escrow_tokens.key != &metadata.escrow_tokens
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let available = metadata.available(now);
    msg!("Available {}", available);
    let escrow_token_info = unpack_token_account(&acc.escrow_tokens)?;
    msg!("Amount {}", escrow_token_info.amount);
    let seeds = [acc.metadata.key.as_ref(), &[nonce]];
    invoke_signed(
        &spl_token::instruction::transfer(
            acc.token_program.key,
            acc.escrow_tokens.key,
            acc.recipient_tokens.key,
            acc.escrow_tokens.key,
            &[],
            available,
        )?,
        &[
            acc.escrow_tokens.clone(),
            acc.recipient_tokens.clone(),
            acc.escrow_tokens.clone(),
            acc.token_program.clone(),
        ],
        &[&seeds],
    )?;
    let escrow_token_info = unpack_token_account(&acc.escrow_tokens)?;
    msg!("Amount {}", escrow_token_info.amount);
    metadata.withdrawn_amount += available;
    let remains = metadata.ix.deposited_amount - metadata.withdrawn_amount;
    msg!(
        "Deposited {} , withdrawn: {}, tokens remain {}",
        metadata.ix.deposited_amount,
        metadata.withdrawn_amount,
        remains
    );
    if remains > 0 {
        invoke_signed(
            &spl_token::instruction::transfer(
                acc.token_program.key,
                acc.escrow_tokens.key,
                acc.sender_tokens.key,
                acc.escrow_tokens.key,
                &[],
                remains,
            )?,
            &[
                acc.escrow_tokens.clone(),
                acc.sender_tokens.clone(),
                acc.escrow_tokens.clone(),
                acc.token_program.clone(),
            ],
            &[&seeds],
        )?;
    }

    let rent_escrow_tokens = acc.escrow_tokens.lamports();

    invoke_signed(
        &spl_token::instruction::close_account(
            acc.token_program.key,
            acc.escrow_tokens.key,
            acc.sender.key,
            acc.escrow_tokens.key,
            &[],
        )?,
        &[
            acc.escrow_tokens.clone(),
            acc.sender.clone(),
            acc.escrow_tokens.clone(),
        ],
        &[&seeds],
    )?;

    if now < metadata.closable_at {
        metadata.last_withdrawn_at = now;
        metadata.canceled_at = now;
    }
    let bytes = metadata.try_to_vec().unwrap();
    data[0..bytes.len()].clone_from_slice(&bytes);

    msg!(
        "Transferred: {} {} tokens",
        encode_base10(available, mint_info.decimals.into()),
        metadata.mint
    );
    msg!(
        "Returned: {} {} tokens",
        encode_base10(remains, mint_info.decimals.into()),
        metadata.mint
    );
    msg!(
        "Returned rent: {} lamports",
        rent_escrow_tokens /* + remains_meta */
    );

    Ok(())
}

pub fn transfer_recipient(program_id: &Pubkey, acc: TransferAccounts) -> ProgramResult {
    msg!("Transferring stream recipient");

    if acc.metadata.data_is_empty()
        || acc.metadata.owner != program_id
        || acc.escrow_tokens.data_is_empty()
        || acc.escrow_tokens.owner != &spl_token::id()
    {
        return Err(ProgramError::UninitializedAccount);
    }

    if !acc.authorized_wallet.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !acc.metadata.is_writable
        || !acc.authorized_wallet.is_writable
        || !acc.new_recipient_tokens.is_writable
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut data = acc.metadata.try_borrow_mut_data()?;
    let mut metadata: TokenStreamData = match solana_borsh::try_from_slice_unchecked(&data) {
        Ok(v) => v,
        Err(_) => return Err(InvalidMetadata.into()),
    };

    if !metadata.ix.transferable_by_recipient && !metadata.ix.transferable_by_sender {
        return Err(TransferNotAllowed.into());
    }

    let mut authorized = false;
    if metadata.ix.transferable_by_recipient && metadata.recipient == *acc.authorized_wallet.key {
        authorized = true;
    }
    if metadata.ix.transferable_by_sender && &metadata.sender == acc.authorized_wallet.key {
        authorized = true;
    }
    if !authorized {
        msg!("Error: Unauthorized wallet");
        return Err(TransferNotAllowed.into());
    }

    let (escrow_tokens_pubkey, _) =
        Pubkey::find_program_address(&[acc.metadata.key.as_ref()], program_id);
    let new_recipient_tokens_key =
        get_associated_token_address(acc.new_recipient.key, acc.mint.key);

    if acc.new_recipient_tokens.key != &new_recipient_tokens_key
        || acc.mint.key != &metadata.mint
        || acc.authorized_wallet.key != &metadata.recipient
        || acc.escrow_tokens.key != &metadata.escrow_tokens
        || acc.escrow_tokens.key != &escrow_tokens_pubkey
        || acc.token_program.key != &spl_token::id()
        || acc.system_program.key != &system_program::id()
        || acc.rent.key != &sysvar::rent::id()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    if !acc.new_recipient_tokens.data_is_empty() {
        let tokens_struct_size = spl_token::state::Account::LEN;
        let cluster_rent = Rent::get()?;
        let tokens_rent = cluster_rent.minimum_balance(tokens_struct_size);

        if acc.authorized_wallet.lamports() < tokens_rent {
            msg!("Error: Insufficient funds in {}", acc.authorized_wallet.key);
            return Err(ProgramError::InsufficientFunds);
        }

        msg!("Initializing new recipient's associated token account");
        invoke(
            &create_associated_token_account(
                acc.authorized_wallet.key,
                acc.new_recipient.key,
                acc.mint.key,
            ),
            &[
                acc.authorized_wallet.clone(),
                acc.new_recipient_tokens.clone(),
                acc.new_recipient.clone(),
                acc.mint.clone(),
                acc.system_program.clone(),
                acc.token_program.clone(),
                acc.rent.clone(),
            ],
        )?;
    }

    metadata.recipient = *acc.new_recipient.key;
    metadata.recipient_tokens = *acc.new_recipient_tokens.key;

    let bytes = metadata.try_to_vec()?;
    data[0..bytes.len()].clone_from_slice(&bytes);

    Ok(())
}

pub fn topup_stream(program_id: &Pubkey, acc: TopUpAccounts, amount: u64) -> ProgramResult {
    msg!("Topping up the escrow account");

    if acc.metadata.data_is_empty() || acc.escrow_tokens.owner != &spl_token::id() {
        return Err(ProgramError::UninitializedAccount);
    }

    if !acc.sender.is_writable
        || !acc.sender_tokens.is_writable
        || !acc.metadata.is_writable
        || !acc.escrow_tokens.is_writable
    {
        return Err(AccountsNotWritable.into());
    }

    let (escrow_tokens_pubkey, _) =
        Pubkey::find_program_address(&[acc.metadata.key.as_ref()], program_id);

    if acc.token_program.key != &spl_token::id() || acc.escrow_tokens.key != &escrow_tokens_pubkey {
        return Err(ProgramError::InvalidAccountData);
    }

    if !acc.sender.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let sender_token_info = unpack_token_account(&acc.sender_tokens)?;

    if &sender_token_info.mint != acc.mint.key {
        return Err(MintMismatch.into());
    }

    if amount == 0 {
        msg!("Error: Amount can't be zero.");
        return Err(ProgramError::InvalidArgument);
    }

    let mut data = acc.metadata.try_borrow_mut_data()?;
    let mut metadata: TokenStreamData = match solana_borsh::try_from_slice_unchecked(&data) {
        Ok(v) => v,
        Err(_) => return Err(InvalidMetadata.into()),
    };

    if acc.mint.key != &metadata.mint || acc.escrow_tokens.key != &metadata.escrow_tokens {
        msg!("Error: Metadata does not match given accounts");
        return Err(ProgramError::InvalidAccountData);
    }

    let now = Clock::get()?.unix_timestamp as u64;
    if metadata.closable() < now {
        msg!("Error: Topup after the stream is closed");
        return Err(StreamClosed.into());
    }

    msg!("Transferring to the escrow account");
    invoke(
        &spl_token::instruction::transfer(
            acc.token_program.key,
            acc.sender_tokens.key,
            acc.escrow_tokens.key,
            acc.sender.key,
            &[],
            amount,
        )?,
        &[
            acc.sender_tokens.clone(),
            acc.escrow_tokens.clone(),
            acc.sender.clone(),
            acc.token_program.clone(),
        ],
    )?;

    metadata.ix.deposited_amount += amount;
    metadata.closable_at = metadata.closable();

    let bytes = metadata.try_to_vec().unwrap();
    data[0..bytes.len()].clone_from_slice(&bytes);

    let mint_info = unpack_mint_account(&acc.mint)?;

    msg!(
        "Successfully topped up {} to token stream {} on behalf of {}",
        encode_base10(amount, mint_info.decimals.into()),
        acc.escrow_tokens.key,
        acc.sender.key,
    );

    Ok(())
}
