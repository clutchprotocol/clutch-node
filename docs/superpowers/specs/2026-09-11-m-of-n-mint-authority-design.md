# M-of-N mint authority

Status: design, for implementation before mainnet genesis.
Date: 2026-09-11.

## The problem

`Mint::verify_state` authorises against a single address:

```rust
if canonical_account_address(from) != canonical_account_address(&params.mint_authority)
```

One key, one signature, unlimited supply. Every other control on minting lives off-chain in
`treasury-service`: the four-eyes approval, the per-transaction cap, the daily cap, the
reconciliation halt breaker. A holder of that key constructs a `Mint` and submits it straight to
the node, and not one of those controls runs. The node has never heard of them.

So at the protocol layer the only control on unbounded minting is possession of one secret. For a
token that is supposed to be backed 1:1, that is the whole peg resting on one file.

## Why this has to be decided before genesis

`mint_authority` is a field of `ChainInit`, which rides in the genesis block and is committed to
the genesis hash that peers compare at handshake. Changing the shape of that struct changes the
hash, which means a new chain. Mainnet is a new genesis anyway. After mainnet launches, this
change costs a chain reset on a live money chain, which is not a thing anyone does.

This is the only item on the readiness list that gets permanently more expensive by waiting.

## Design

Authorise a `Mint` when the transaction is signed by one member of an authority set **and**
carries `threshold - 1` further signatures from distinct other members.

### Where the extra signatures live

Inside the `Mint` arguments, not in the transaction envelope.

The envelope is `[from, nonce, chain_id, r, s, v, hash, data]` and is shared by every transaction
type. Widening it to hold several signatures would touch every transaction, the SDK, and the
encoding spec, to serve one transaction type that the SDK does not even build. Putting them in the
`Mint` arguments confines the change to `Mint` and `ChainInit`.

### What the cosigners actually sign

**Not the transaction hash.** The transaction hash covers the `Mint` arguments, and the
cosignatures are in those arguments, so a cosigner signing the transaction hash would have to sign
something that includes their own signature. That is circular and has no fixed point.

Cosigners sign a separate **mint approval digest**:

```
approval_digest = keccak256(rlp([chain_id, to, amount, credit_ref]))
```

expressed as 64 lowercase hex characters without `0x`, then signed with the stack's existing
convention — `SignatureKeys::sign(secret, approval_digest_hex.as_bytes())`, which keccaks the hex
string's UTF-8 bytes. That is the same convention `ChainSigner::sign_hash_hex` already implements,
so a cosigner needs no new signing primitive and any KMS-backed signer works unchanged.

The four fields are exactly what an approver is agreeing to: this much CLT, to this account, for
this off-chain payment, on this chain. Deliberately **excluded**:

- `nonce` and `from`, so approvers do not need to know which authority will submit, or in what
  order. They approve a mint, not a transaction. This matches how the four-eyes flow already
  works, where approvers act before anyone assembles a transaction.
- The cosignature list itself, which is what makes the digest well-defined.

Replay is already closed by `credit_ref`, which is exactly-once in state
(`processed_ref_key`). A stolen approval signature cannot be reused, because the second mint
carrying that `credit_ref` is rejected whatever its signatures. `chain_id` is in the digest, so an
approval from one chain is not valid on another.

### What the envelope signer commits to

The transaction hash covers the `Mint` arguments **including** the cosignature list. So the
submitting authority signs the exact set of cosignatures. Nobody can strip one, add one, or swap
one after the envelope is signed without invalidating it. Ordering therefore is: collect
cosignatures, build the `Mint` containing them, then sign the envelope.

### Verification

In `Mint::verify_state`, in this order:

1. `from` is a member of the authority set.
2. Every cosignature recovers to a member of the set.
3. All signers are distinct — including `from`, so one key cannot sign twice.
4. `1 + cosignatures.len() >= threshold`.

Then the existing checks (`to` valid, amount, supply ceiling, `credit_ref` validity and
exactly-once) run unchanged.

## Configuration

`ChainInit` gains two fields:

| Field | Meaning |
|---|---|
| `mint_cosigners: Vec<String>` | Further authorised addresses. The full set is `{mint_authority} ∪ mint_cosigners`. |
| `mint_threshold: u8` | Signatures required. `0` and `1` both mean today's behaviour. |

A 2-of-3 is `mint_authority = A`, `mint_cosigners = [B, C]`, `mint_threshold = 2`. Any two of the
three can then mint: the submitter is whichever holds the nonce, and the set membership test does
not care which.

Validated at boot, beside the existing economics asserts: threshold at most the set size (a
threshold nothing can satisfy bricks minting permanently), no duplicate members, every member a
valid address.

## Backward compatibility

Both new fields are appended to the `ChainInit` RLP **only when `mint_threshold > 1`**, and the
decoder accepts either 8 items or 10. A chain that does not use M-of-N therefore encodes
byte-identically to today and keeps its genesis hash.

That matters because the stage chain is running right now. It keeps running, untouched, and
mainnet genesis opts in. Same for `Mint`: the cosignature list is a fourth RLP item present only
when non-empty, and the decoder accepts 3 or 4.

## Why not the alternatives

**Threshold signatures (MPC/TSS).** Produces one ordinary secp256k1 signature from shares, so the
chain needs no change at all. Rejected because operating a TSS protocol — key refresh, abort
handling, share storage — is a larger new risk surface for a solo maintainer than this change is,
and it puts the security property in a library rather than in consensus.

**On-chain multisig contract.** There is no contract layer. The ride lifecycle is transaction
types, not smart contracts.

**Keep four-eyes off-chain, harden the key instead.** This is the fallback if M-of-N is not built,
and it is strictly weaker: a stolen key still bypasses four-eyes entirely. Non-exportable custody
lowers the probability of theft; M-of-N lowers the consequence.

## What this does not solve

If one person holds all N keys, this gives multi-*place* control, not multi-*person* control. An
attacker must breach several stores instead of one file, which is a real gain, but a compromised
operator still mints. A second human stays open as readiness item G3, and the public disclosure
that one operator can mint stays necessary until there is one.

## Implementation order

1. `clutch-node`: `SignatureKeys::recover_address`, `ChainInit` fields and RLP, `Mint`
   cosignatures and verification, boot validation, tests.
2. `clutch-treasury`: the four-eyes approval becomes a real signature. Each approver's row gains
   the approval signature over the digest, and the mint submitter assembles them. This is the
   change that turns "an attacker with database access can fake an approval" into "they cannot".
3. Genesis configuration for mainnet, with the chosen N and threshold.

Step 1 is self-contained and ships first; the chain accepts single-signature mints throughout, so
nothing breaks between steps.
