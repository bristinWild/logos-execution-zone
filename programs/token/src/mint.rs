use nssa_core::{
    account::{Account, AccountWithMetadata, Data},
    program::{AccountPostState, Claim},
};
use token_core::{TokenDefinition, TokenHolding};

#[must_use]
pub fn mint(
    definition_account: AccountWithMetadata,
    user_holding_account: AccountWithMetadata,
    amount_to_mint: u128,
) -> Vec<AccountPostState> {
    assert!(
        definition_account.is_authorized,
        "Definition authorization is missing; only the mint authority can mint"
    );

    let mut definition = TokenDefinition::try_from(&definition_account.account.data)
        .expect("Token Definition account must be valid");

    // LP-0013: enforce mint authority — minting is only allowed if mint_authority is Some.
    // The is_authorized check above ensures the caller controls the definition account,
    // which serves as proof they hold the mint authority key.
    if let TokenDefinition::Fungible { mint_authority, .. } = &definition {
        assert!(
            mint_authority.is_some(),
            "Mint authority has been revoked; this token has a fixed supply"
        );
    }

    let mut holding = if user_holding_account.account == Account::default() {
        TokenHolding::zeroized_from_definition(definition_account.account_id, &definition)
    } else {
        TokenHolding::try_from(&user_holding_account.account.data)
            .expect("Token Holding account must be valid")
    };

    assert_eq!(
        definition_account.account_id,
        holding.definition_id(),
        "Mismatch Token Definition and Token Holding"
    );

    match (&mut definition, &mut holding) {
        (
            TokenDefinition::Fungible {
                name: _,
                metadata_id: _,
                mint_authority: _,
                total_supply,
            },
            TokenHolding::Fungible {
                definition_id: _,
                balance,
            },
        ) => {
            *balance = balance
                .checked_add(amount_to_mint)
                .expect("Balance overflow on minting");

            *total_supply = total_supply
                .checked_add(amount_to_mint)
                .expect("Total supply overflow");
        }
        (
            TokenDefinition::NonFungible { .. },
            TokenHolding::NftMaster { .. } | TokenHolding::NftPrintedCopy { .. },
        ) => {
            panic!("Cannot mint additional supply for Non-Fungible Tokens");
        }
        _ => panic!("Mismatched Token Definition and Token Holding types"),
    }

    let mut definition_post = definition_account.account;
    definition_post.data = Data::from(&definition);

    let mut holding_post = user_holding_account.account;
    holding_post.data = Data::from(&holding);

    vec![
        AccountPostState::new(definition_post),
        AccountPostState::new_claimed_if_default(holding_post, Claim::Authorized),
    ]
}
