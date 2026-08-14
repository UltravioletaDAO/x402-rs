#!/usr/bin/env bash
#
# EIP-7702: does a 2300-gas transfer()/send() still reach an EOA that has been
# delegated with a REAL type-4 transaction?
#
# WHY THIS SCRIPT EXISTS
#
# Execution Market's FeedbackDelegate carries `receive() external payable {}` so
# that delegating a rater's EOA does not stop the wallet from receiving ETH. The
# question was whether that is enough: under EIP-7702 the account stores
# `0xef0100 || delegate` and the EVM has to LOAD the delegate's code before
# running it, so the fear was that the cold account access (2600 gas) would be
# taken out of the 2300 stipend and every `transfer()` into a delegated wallet
# would start failing.
#
# It could not be settled with `hardhat_setCode`, which writes the runtime code
# straight into the account so the EVM never pays to load it -- EM marked their
# own test NOT CONCLUSIVE for exactly that reason instead of claiming a result.
# This measures it against a node with Prague enabled and a real type-4
# delegation.
#
# RESULT (2026-08-14, anvil 1.4.4, --hardfork prague, foundry 1.4.4):
#
#   case                                        send() ok   gas charged
#   ------------------------------------------  ---------   -----------
#   delegated -> FeedbackDelegate, delegate COLD    true        12050
#   delegated -> FeedbackDelegate, delegate WARM    true         9550
#   delegated -> delegate with NO payable receive   false       12039
#
#   transfer() to the delegated account: SUCCEEDS (12070 gas charged)
#
# The cold/warm delta is 2500, which is exactly EIP-2929's cold (2600) minus
# warm (100). So the account-access charge for loading the delegate's code is
# billed to the CALLER's frame; it is not taken out of the callee's 2300
# stipend, and `receive()` still runs. The mitigation holds.
#
# The negative control is the part that makes the positive result mean anything:
# an account delegated to a contract WITHOUT a payable receive does fail, so the
# harness can tell success from failure.
#
# GOTCHA found while building this: `cast send --auth <delegate> <self>` sends
# the type-4 transaction TO the account being delegated, which makes it the
# first call into the new code. If the delegate has no receive/fallback, that
# transaction reverts and the delegation never lands. Send it to a third party
# instead.
#
# Usage: scripts/spike_eip7702_stipend.sh   (starts and stops its own anvil)

set -euo pipefail

PORT="${PORT:-8555}"
RPC="http://127.0.0.1:${PORT}"
FOUNDRY_BIN="${FOUNDRY_BIN:-$HOME/.foundry/bin}"
export PATH="$PATH:$FOUNDRY_BIN"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; [ -n "${ANVIL_PID:-}" ] && kill "$ANVIL_PID" 2>/dev/null || true' EXIT

# anvil's deterministic accounts
A_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
A_ADDR=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266   # sponsor / payer of gas
B_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
B_ADDR=0x70997970C51812dc3A010C7d01b50e0d17dc79C8   # rater, delegated to the real delegate
C_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
C_ADDR=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC   # negative control

# Where FeedbackDelegate.sol and its OpenZeppelin dependency live. Override both
# if the sibling checkout is somewhere else.
EM_CONTRACTS="${EM_CONTRACTS:-/mnt/z/ultravioleta/dao/execution-market/contracts}"

say() { printf '\n=== %s ===\n' "$1"; }

mkdir -p "$WORK/src"
cat > "$WORK/foundry.toml" <<EOF
[profile.default]
src = "src"
out = "out"
libs = []
evm_version = "prague"
remappings = ["@openzeppelin/=${EM_CONTRACTS}/node_modules/@openzeppelin/"]
EOF

cat > "$WORK/src/Probes.sol" <<'SOL'
// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// A delegate with NO payable receive. The negative control: an account
/// delegated here must stop accepting plain transfers.
contract NoReceiveDelegate {
    function nothing() external pure returns (uint256) {
        return 1;
    }
}

/// Sends value the three ways that matter and reports what happened.
contract Payer {
    event Result(string what, bool ok, uint256 gasUsed);

    /// Exactly 2300 gas, reverts on failure. The one EIP-7702 put in question.
    function payTransfer(address payable to) external payable {
        uint256 before = gasleft();
        to.transfer(msg.value);
        emit Result("transfer", true, before - gasleft());
    }

    /// Same stipend, returns false instead of reverting, so failure is
    /// observable rather than fatal.
    function paySend(address payable to) external payable returns (bool ok) {
        uint256 before = gasleft();
        ok = to.send(msg.value);
        emit Result("send", ok, before - gasleft());
    }

    /// Warm the delegate address (EIP-2929) before sending, to separate the
    /// cold-access cost from the stipend.
    function paySendWarm(address payable to, address warm) external payable returns (bool ok) {
        uint256 sink = warm.balance;
        sink;
        uint256 before = gasleft();
        ok = to.send(msg.value);
        emit Result("send-warm", ok, before - gasleft());
    }

    receive() external payable {}
}
SOL

cp "$EM_CONTRACTS/contracts/FeedbackDelegate.sol" "$WORK/src/"

say "building probes + FeedbackDelegate (evm_version = prague)"
(cd "$WORK" && forge build >/dev/null)

say "starting anvil --hardfork prague on port $PORT"
anvil --hardfork prague --port "$PORT" --silent >/dev/null 2>&1 &
ANVIL_PID=$!
sleep 4
cast chain-id --rpc-url "$RPC" >/dev/null || { echo "[FAIL] anvil did not come up"; exit 1; }
# EIP-7685 requestsHash in the header is Prague's fingerprint.
if cast rpc eth_getBlockByNumber latest false --rpc-url "$RPC" | grep -q requestsHash; then
  echo "[OK] chain reports a Prague header (requestsHash present)"
else
  echo "[FAIL] no requestsHash in the header: this chain is not on Prague"; exit 1
fi

deployed() { python3 -c 'import json,sys;print(json.load(sys.stdin)["deployedTo"])'; }
say "deploying"
cd "$WORK"
NORECV=$(forge create --rpc-url "$RPC" --private-key $A_KEY src/Probes.sol:NoReceiveDelegate --broadcast --json | deployed)
PAYER=$(forge create --rpc-url "$RPC" --private-key $A_KEY src/Probes.sol:Payer --broadcast --json | deployed)
FBD=$(forge create --rpc-url "$RPC" --private-key $A_KEY src/FeedbackDelegate.sol:FeedbackDelegate \
        --broadcast --json --constructor-args 0x8004BAa17C55a88189AE136b182e5fdA19dE9b63 | deployed)
echo "  FeedbackDelegate  $FBD"
echo "  NoReceiveDelegate $NORECV"
echo "  Payer             $PAYER"
cast send --rpc-url "$RPC" --private-key $A_KEY "$PAYER" --value 10ether >/dev/null

report() {
  python3 -c '
import json,sys
r=json.load(sys.stdin)
print("    tx status:", r["status"], " tx gasUsed:", int(r["gasUsed"],16))
for l in r.get("logs",[]):
    d=l["data"][2:]; w=[d[i:i+64] for i in range(0,len(d),64)]
    print("    Result: ok =", bool(int(w[1],16)), " gas charged for the send =", int(w[2],16))
'
}

say "real type-4 delegation: rater -> FeedbackDelegate"
cast send --rpc-url "$RPC" --private-key $B_KEY --auth "$FBD" "$B_ADDR" --json >/dev/null
CODE_B=$(cast code "$B_ADDR" --rpc-url "$RPC")
echo "  code(rater) = $CODE_B"
case "$CODE_B" in
  0xef0100*) echo "  [OK] delegation designator installed (0xef0100 || delegate)";;
  *) echo "  [FAIL] no delegation designator: nothing below would mean anything"; exit 1;;
esac

say "MEASUREMENT 1 - send() with the delegate COLD"
cast send --rpc-url "$RPC" --private-key $A_KEY "$PAYER" "paySend(address)" "$B_ADDR" --value 1 --json | report

say "MEASUREMENT 2 - send() with the delegate WARMED first"
cast send --rpc-url "$RPC" --private-key $A_KEY "$PAYER" "paySendWarm(address,address)" "$B_ADDR" "$FBD" --value 1 --json | report

say "MEASUREMENT 3 - transfer() (reverts on failure) to the delegated account"
cast send --rpc-url "$RPC" --private-key $A_KEY "$PAYER" "payTransfer(address)" "$B_ADDR" --value 1 --json | report

say "NEGATIVE CONTROL - an account delegated to a contract with NO payable receive"
# Sent to a THIRD PARTY on purpose: sending the type-4 tx to the account itself
# is the first call into the new code, and this delegate would reject it.
cast send --rpc-url "$RPC" --private-key $C_KEY --auth "$NORECV" "$A_ADDR" --json >/dev/null
echo "  code(control) = $(cast code "$C_ADDR" --rpc-url "$RPC")"
cast send --rpc-url "$RPC" --private-key $A_KEY "$PAYER" "paySend(address)" "$C_ADDR" --value 1 --json | report
echo "  ^ ok = False is the point: the harness can tell failure from success,"
echo "    so 'it worked' above is a measurement and not an artefact."
