Hey Ali,

Your Shanghai recompile left TSTORE/TLOAD inside the PaymentOperator function bodies. The factory dispatcher works (selectors are present and factories deploy fine), but `release()`, `refundInEscrow()`, and other lifecycle functions hit TSTORE/TLOAD during execution and revert on SKALE.

`authorize()` works because its code path avoids the reentrancy guard. We confirmed this by deploying operators, locking funds successfully, and then failing on release.

## What we verified

- Lock TX `0xf49b66b4...a93a` succeeded -- 0.05 USDC locked via `authorize()`
- Same PaymentInfo used for `release()` with correct new selector `0xc602dd4a`
- Authorization not expired (1.7h remaining at time of test)
- Simulated via `eth_call` -- reverts with no error message
- Real TX -- also reverts, consuming all gas

## Root cause

The Shanghai downgrade removed TSTORE/TLOAD from the **factory** bytecode (so factories deploy fine on SKALE), but NOT from the **PaymentOperator** bytecode that factories produce. The operator still has `ReentrancyGuardTransient` in the release/refund/charge code paths.

The result: `authorize()` works, but `release()`, `refundInEscrow()`, `charge()`, and any function that enters the reentrancy guard reverts on SKALE.

## Fix needed

Replace `ReentrancyGuardTransient` (transient storage, TSTORE/TLOAD) with `ReentrancyGuard` (regular storage) in `PaymentOperator.sol` for the SKALE build. Recompile and redeploy the PaymentOperatorFactory on SKALE so it produces operators with the storage-based guard.

## Stuck funds

- 0.10 USDC across 2 lockboxes on SKALE Base via operator `0x28c23AE8f55aDe5Ea10a5353FC40418D0c1B3d33`
- Cannot be released until operator is fixed or SKALE upgrades to Cancun

Let me know if you need anything from our side.
