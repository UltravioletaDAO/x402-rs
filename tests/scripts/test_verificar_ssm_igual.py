#!/usr/bin/env python3
"""Tests for scripts/verificar_ssm_igual.py, against a stand-in for AWS: no credentials, no network.

    python3 tests/scripts/test_verificar_ssm_igual.py

Every value is generated here at random, so no key, token or URL lives in this file, and every
test that prints checks that none of them -- nor a piece of one, nor its length -- reaches the
output. The address tests need eth_account / cryptography and skip without them.
"""

import contextlib
import importlib.util
import io
import json
import os
import secrets
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts"))

import verificar_ssm_igual as v  # noqa: E402

# Assembled, so no line of this file is an account-bearing ARN or a secret name with its suffix
# (.github/workflows/no-account-id.yml).
ACCOUNT = "1111" + "22223333"
SUFFIX = "Q7" + "x9Zk"
SM = "arn:aws:secretsmanager:us-east-2:" + ACCOUNT + ":secret:"
SSM = "arn:aws:ssm:us-east-2:" + ACCOUNT + ":parameter"
KEY = "alias/facilitator-bazaar-admin-token"
IMAGE = ACCOUNT + ".dkr.ecr.us-east-2.amazonaws.com/facilitator:2.51.0-abc1234"


def rand() -> str:
    # 59 characters: a length nothing else in the output is likely to print.
    return secrets.token_urlsafe(44)


class Fake:
    """Secrets Manager, SSM and ECS in dictionaries. Raises the script's own exceptions."""

    def __init__(self):
        self.secrets = {}    # secret ARN -> SecretString
        self.params = {}     # parameter ARN -> value
        self.meta = {}       # parameter name -> (Type, KeyId)
        self.tds = {}        # family:revision -> containerDefinitions
        self.fail = {}       # ARN or name -> error code to raise
        self.reads = []

    def secret(self, secret_id, stage, version):
        self.reads.append(secret_id)
        if secret_id in self.fail:
            raise v.ReadFailed(self.fail[secret_id])
        if secret_id not in self.secrets:
            raise v.NotFound("ResourceNotFoundException")
        return self.secrets[secret_id]

    def parameter(self, name):
        self.reads.append(name)
        if name in self.fail:
            raise v.ReadFailed(self.fail[name])
        if name not in self.params:
            raise v.NotFound("ParameterNotFound")
        return self.params[name]

    def metadata(self, name):
        if name in self.fail:
            raise v.ReadFailed(self.fail[name])
        return self.meta.get(name)

    def task_definition(self, spec):
        if spec not in self.tds:
            raise v.NotFound("ClientException")
        return self.tds[spec]


class NoValues(Fake):
    def secret(self, *args):
        raise AssertionError("--solo-metadatos read a secret value")

    def parameter(self, *args):
        raise AssertionError("--solo-metadatos read a parameter value")


def world(fake=None):
    """The production shapes: a JSON field, two fields of one secret, a whole string, an RPC
    field, and BAZAAR_ADMIN_TOKEN, which was on SSM before and stays on the same parameter."""
    f = fake or Fake()
    evm, near_key, near_id, stellar, rpc, other_rpc, bazaar = (rand() for _ in range(7))
    evm_s, near_s = SM + "facilitator-evm-mainnet-private-key-" + SUFFIX, SM + "facilitator-near-mainnet-keypair-" + SUFFIX
    stellar_s, rpc_s = SM + "facilitator-stellar-keypair-mainnet-" + SUFFIX, SM + "facilitator-rpc-mainnet-" + SUFFIX
    f.secrets[evm_s] = json.dumps({"private_key": evm})
    f.secrets[near_s] = json.dumps({"private_key": near_key, "account_id": near_id})
    f.secrets[stellar_s] = stellar
    f.secrets[rpc_s] = json.dumps({"base": rpc, "polygon": other_rpc})
    pairs = {
        "EVM_PRIVATE_KEY_MAINNET": (evm_s + ":private_key::", "/facilitator/evm-mainnet-private-key/private_key", evm),
        "NEAR_PRIVATE_KEY_MAINNET": (near_s + ":private_key::", "/facilitator/near-mainnet-keypair/private_key", near_key),
        "NEAR_ACCOUNT_ID_MAINNET": (near_s + ":account_id::", "/facilitator/near-mainnet-keypair/account_id", near_id),
        "STELLAR_PRIVATE_KEY_MAINNET": (stellar_s, "/facilitator/stellar-keypair-mainnet", stellar),
        "RPC_URL_BASE": (rpc_s + ":base::", "/facilitator/rpc-mainnet/base", rpc),
    }
    old, new = [], []
    for variable, (value_from, name, value) in pairs.items():
        old.append({"name": variable, "valueFrom": value_from})
        new.append({"name": variable, "valueFrom": SSM + name})
        f.params[SSM + name] = value
        f.meta[name] = ("SecureString", KEY)
    bazaar_arn = SSM + "/facilitator/bazaar-admin-token"
    old.append({"name": "BAZAAR_ADMIN_TOKEN", "valueFrom": bazaar_arn})
    new.append({"name": "BAZAAR_ADMIN_TOKEN", "valueFrom": bazaar_arn})
    f.params[bazaar_arn] = bazaar
    f.meta["/facilitator/bazaar-admin-token"] = ("SecureString", KEY)
    env = [{"name": "ESCROW_LIFECYCLE_AUTH", "value": "log"}, {"name": "PORT", "value": "8080"}]
    old_td = [{"name": "facilitator", "image": IMAGE, "environment": env, "secrets": old}]
    new_td = [{"name": "facilitator", "image": IMAGE, "environment": [dict(e) for e in env], "secrets": new}]
    return f, old_td, new_td, [evm, near_key, near_id, stellar, rpc, other_rpc, bazaar]


def run(fake, old_td, new_td, *extra, old_doc=None, new_doc=None):
    with tempfile.TemporaryDirectory() as tmp:
        old_path, new_path = os.path.join(tmp, "viejo.json"), os.path.join(tmp, "nuevo.json")
        with open(old_path, "w", encoding="utf-8") as handle:
            json.dump(old_doc if old_doc is not None else old_td, handle)
        with open(new_path, "w", encoding="utf-8") as handle:
            json.dump(new_doc if new_doc is not None else new_td, handle)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = v.main(["--viejo", old_path, "--nuevo", new_path, *extra], sources=fake)
    return code, out.getvalue()


def row(out, variable):
    found = [line for line in out.splitlines() if f"| {variable} |" in line]
    if len(found) != 1:
        raise AssertionError(f"expected one row for {variable}, got {found}")
    return found[0]


class Base(unittest.TestCase):
    def assertNoLeak(self, out, values):
        for value in values:
            self.assertNotIn(value, out)
            self.assertNotIn(value[:6], out)
            self.assertNotIn(value[-6:], out)
            self.assertNotIn(str(len(value)), out)
        self.assertNotIn(ACCOUNT, out)
        self.assertNotIn(SUFFIX, out)


class Verdicts(Base):
    def test_every_copy_equal_is_exit_0_and_prints_no_value(self):
        f, old_td, new_td, values = world()
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 0, out)
        for variable in ("EVM_PRIVATE_KEY_MAINNET", "NEAR_PRIVATE_KEY_MAINNET", "NEAR_ACCOUNT_ID_MAINNET",
                         "STELLAR_PRIVATE_KEY_MAINNET", "RPC_URL_BASE"):
            self.assertTrue(row(out, variable).startswith("| ok |"), row(out, variable))
        self.assertTrue(row(out, "BAZAAR_ADMIN_TOKEN").startswith("| sin cambio |"))
        self.assertIn("RESULTADO: todo igual", out)
        self.assertIn("SM facilitator-near-mainnet-keypair:account_id", row(out, "NEAR_ACCOUNT_ID_MAINNET"))
        self.assertIn("SSM /facilitator/near-mainnet-keypair/account_id", row(out, "NEAR_ACCOUNT_ID_MAINNET"))
        self.assertNoLeak(out, values)

    def test_one_trailing_newline_is_a_difference(self):
        f, old_td, new_td, values = world()
        name = SSM + "/facilitator/rpc-mainnet/base"
        f.params[name] += "\n"
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "RPC_URL_BASE").startswith("| DIFIERE |"))
        self.assertTrue(row(out, "EVM_PRIVATE_KEY_MAINNET").startswith("| ok |"))
        self.assertNoLeak(out, values + [f.params[name]])

    def test_the_other_field_of_the_same_secret_is_a_difference(self):
        f, old_td, new_td, values = world()
        f.params[SSM + "/facilitator/rpc-mainnet/base"] = values[5]  # the polygon field
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "RPC_URL_BASE").startswith("| DIFIERE |"))
        self.assertNoLeak(out, values)

    def test_a_parameter_that_does_not_exist(self):
        f, old_td, new_td, values = world()
        del f.params[SSM + "/facilitator/stellar-keypair-mainnet"]
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "STELLAR_PRIVATE_KEY_MAINNET").startswith("| FALTA |"))
        self.assertNoLeak(out, values)

    def test_a_parameter_ssm_does_not_describe(self):
        f, old_td, new_td, values = world()
        del f.meta["/facilitator/stellar-keypair-mainnet"]
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "STELLAR_PRIVATE_KEY_MAINNET").startswith("| PARAMETRO INEXISTENTE |"))

    def test_a_field_missing_from_the_secret(self):
        f, old_td, new_td, values = world()
        f.secrets[SM + "facilitator-near-mainnet-keypair-" + SUFFIX] = json.dumps({"private_key": values[1]})
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "NEAR_ACCOUNT_ID_MAINNET").startswith("| FALTA EN EL VIEJO |"))
        self.assertTrue(row(out, "NEAR_PRIVATE_KEY_MAINNET").startswith("| ok |"))
        self.assertNoLeak(out, values)

    def test_a_field_that_is_not_text_has_no_verdict(self):
        f, old_td, new_td, values = world()
        f.secrets[SM + "facilitator-evm-mainnet-private-key-" + SUFFIX] = json.dumps({"private_key": 7})
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 2)
        self.assertTrue(row(out, "EVM_PRIVATE_KEY_MAINNET").startswith("| ERROR |"))

    def test_a_secret_that_is_not_json_quotes_nothing(self):
        f, old_td, new_td, values = world()
        broken = "{" + values[0]
        f.secrets[SM + "facilitator-evm-mainnet-private-key-" + SUFFIX] = broken
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 2)
        self.assertIn("secret is not JSON", row(out, "EVM_PRIVATE_KEY_MAINNET"))
        self.assertNoLeak(out, values + [broken])

    def test_access_denied_is_no_verdict(self):
        f, old_td, new_td, values = world()
        f.fail[SSM + "/facilitator/rpc-mainnet/base"] = "AccessDeniedException"
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 2)
        self.assertIn("AccessDeniedException", row(out, "RPC_URL_BASE"))
        self.assertIn("sin veredicto", out)
        self.assertNoLeak(out, values)

    def test_metadata_access_denied_is_no_verdict(self):
        f, old_td, new_td, values = world()
        f.fail["/facilitator/rpc-mainnet/base"] = "AccessDeniedException"
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 2)
        self.assertTrue(row(out, "RPC_URL_BASE").startswith("| ERROR |"))

    def test_a_difference_outweighs_a_failed_read(self):
        f, old_td, new_td, values = world()
        f.fail[SSM + "/facilitator/rpc-mainnet/base"] = "ThrottlingException"
        f.params[SSM + "/facilitator/stellar-keypair-mainnet"] = rand()
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)


class Shape(Base):
    def test_a_name_dropped_or_added(self):
        f, old_td, new_td, values = world()
        new_td[0]["secrets"] = [s for s in new_td[0]["secrets"] if s["name"] != "RPC_URL_BASE"]
        new_td[0]["secrets"].append({"name": "RPC_URL_EXTRA", "valueFrom": SSM + "/facilitator/rpc-mainnet/extra"})
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "RPC_URL_BASE").startswith("| SOLO EN EL VIEJO |"))
        self.assertTrue(row(out, "RPC_URL_EXTRA").startswith("| SOLO EN EL NUEVO |"))

    def test_two_different_parameters_are_compared_not_skipped(self):
        # Only the SAME valueFrom on both sides is "sin cambio"; SSM -> another SSM is read and compared.
        f, old_td, new_td, values = world()
        moved = SSM + "/facilitator/bazaar-admin-token-v2"
        f.params[moved] = rand()
        f.meta["/facilitator/bazaar-admin-token-v2"] = ("SecureString", KEY)
        for s in new_td[0]["secrets"]:
            if s["name"] == "BAZAAR_ADMIN_TOKEN":
                s["valueFrom"] = moved
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "BAZAAR_ADMIN_TOKEN").startswith("| DIFIERE |"))
        f.params[moved] = values[6]
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 0, out)
        self.assertTrue(row(out, "BAZAAR_ADMIN_TOKEN").startswith("| ok |"))
        self.assertNoLeak(out, values)

    def test_a_variable_still_on_secrets_manager(self):
        f, old_td, new_td, values = world()
        for s in new_td[0]["secrets"]:
            if s["name"] == "RPC_URL_BASE":
                s["valueFrom"] = SM + "facilitator-rpc-mainnet-" + SUFFIX + ":base::"
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "RPC_URL_BASE").startswith("| SIGUE EN SECRETS MANAGER |"))

    def test_an_environment_value_that_moved(self):
        # The trap of a targeted apply that drops a -var: a value back to its default.
        f, old_td, new_td, values = world()
        moved = rand()
        new_td[0]["environment"][0]["value"] = moved
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertIn("DIFIEREN: ESCROW_LIFECYCLE_AUTH", out)
        self.assertNoLeak(out, values + [moved])

    def test_an_environment_variable_that_disappeared(self):
        f, old_td, new_td, values = world()
        new_td[0]["environment"] = new_td[0]["environment"][1:]
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertIn("DIFIEREN: ESCROW_LIFECYCLE_AUTH", out)

    def test_another_image(self):
        f, old_td, new_td, values = world()
        new_td[0]["image"] = IMAGE.replace("2.51.0", "1.47.0")
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertIn("imagen: DIFIERE", out)


class Metadata(Base):
    def test_only_metadata_reads_no_value(self):
        f, old_td, new_td, values = world(NoValues())
        code, out = run(f, old_td, new_td, "--solo-metadatos")
        self.assertEqual(code, 0, out)
        self.assertEqual(f.reads, [])
        self.assertTrue(row(out, "EVM_PRIVATE_KEY_MAINNET").startswith("| omitido |"))
        self.assertIn("ningun valor comparado", out)
        self.assertNotIn("todo igual", out)

    def test_the_default_aws_ssm_key_is_refused(self):
        f, old_td, new_td, values = world(NoValues())
        f.meta["/facilitator/evm-mainnet-private-key/private_key"] = ("SecureString", "alias/aws/ssm")
        code, out = run(f, old_td, new_td, "--solo-metadatos")
        self.assertEqual(code, 1)
        self.assertTrue(row(out, "EVM_PRIVATE_KEY_MAINNET").startswith("| OTRA LLAVE KMS |"))

    def test_the_key_by_its_alias_arn_is_accepted(self):
        f, old_td, new_td, values = world(NoValues())
        f.meta["/facilitator/rpc-mainnet/base"] = ("SecureString", "arn:aws:kms:us-east-2:" + ACCOUNT + ":" + KEY)
        code, out = run(f, old_td, new_td, "--solo-metadatos")
        self.assertEqual(code, 0, out)
        self.assertNoLeak(out, values)

    def test_a_plain_string_parameter_is_refused(self):
        f, old_td, new_td, values = world(NoValues())
        f.meta["/facilitator/rpc-mainnet/base"] = ("String", "")
        code, out = run(f, old_td, new_td, "--solo-metadatos")
        self.assertEqual(code, 1)
        self.assertIn("no es SecureString", row(out, "RPC_URL_BASE"))


class Inputs(Base):
    def test_a_plan_and_a_describe_output(self):
        f, old_td, new_td, values = world()
        plan = {"resource_changes": [
            {"address": "aws_ecs_service.facilitator", "change": {"after": {}}},
            {"address": "aws_ecs_task_definition.facilitator",
             "change": {"after": {"container_definitions": json.dumps(new_td)}}},
        ]}
        code, out = run(f, old_td, new_td, old_doc={"taskDefinition": {"containerDefinitions": old_td}},
                        new_doc=plan)
        self.assertEqual(code, 0, out)

    def test_a_family_revision_is_read_from_ecs(self):
        f, old_td, new_td, values = world()
        f.tds["facilitator-production:478"] = old_td
        with tempfile.TemporaryDirectory() as tmp:
            new_path = os.path.join(tmp, "nuevo.json")
            with open(new_path, "w", encoding="utf-8") as handle:
                json.dump(new_td, handle)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = v.main(["--viejo", "facilitator-production:478", "--nuevo", new_path], sources=f)
            self.assertEqual(code, 0, out.getvalue())
            with contextlib.redirect_stdout(io.StringIO()):
                code = v.main(["--viejo", "facilitator-production:1", "--nuevo", new_path], sources=f)
            self.assertEqual(code, 2)

    def test_value_from_shapes(self):
        whole = v.parse_value_from(SM + "facilitator-sui-keypair-mainnet-" + SUFFIX)
        self.assertEqual((whole.kind, whole.name, whole.field), ("sm", "facilitator-sui-keypair-mainnet", ""))
        self.assertEqual(whole.ref, SM + "facilitator-sui-keypair-mainnet-" + SUFFIX)
        field = v.parse_value_from(SM + "facilitator-rpc-testnet-" + SUFFIX + ":solana-devnet:AWSPREVIOUS:")
        self.assertEqual((field.field, field.stage, field.version), ("solana-devnet", "AWSPREVIOUS", ""))
        self.assertEqual(field.ref, SM + "facilitator-rpc-testnet-" + SUFFIX)
        param = v.parse_value_from(SSM + "/facilitator/rpc-testnet/solana-devnet")
        self.assertEqual((param.kind, param.name), ("ssm", "/facilitator/rpc-testnet/solana-devnet"))
        self.assertEqual(param.ref, SSM + "/facilitator/rpc-testnet/solana-devnet")
        flat = v.parse_value_from(SSM + "/flat-name")
        self.assertEqual(flat.name, "flat-name")
        by_name = v.parse_value_from("/facilitator/bazaar-admin-token")
        self.assertEqual((by_name.kind, by_name.name), ("ssm", "/facilitator/bazaar-admin-token"))

    def test_an_unsupported_value_from_is_no_verdict_without_the_account(self):
        f, old_td, new_td, values = world()
        new_td[0]["secrets"][0]["valueFrom"] = "arn:aws:kms:us-east-2:" + ACCOUNT + ":key/1f1f1f1f"
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 2)
        self.assertIn("unsupported valueFrom", out)
        self.assertNoLeak(out, values)

    def test_an_input_that_is_not_a_task_definition_is_no_verdict(self):
        f, old_td, new_td, values = world()
        self.assertEqual(run(f, old_td, new_td, new_doc={"something": "else"})[0], 2)
        self.assertEqual(run(f, old_td, new_td, "--contenedor", "otro")[0], 2)

    def test_an_unforeseen_error_is_no_verdict_and_prints_only_its_type(self):
        # A secrets entry with no valueFrom is a KeyError inside compare: exit 2, never 1 ("differs").
        f, old_td, new_td, values = world()
        del new_td[0]["secrets"][0]["valueFrom"]
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 2)
        self.assertIn("error inesperado: KeyError", out)
        self.assertNoLeak(out, values)

    def test_metadata_follows_next_token_before_saying_missing(self):
        class Boom(Exception):
            pass

        class Ssm:
            def __init__(self):
                self.calls = []

            def describe_parameters(self, **kwargs):
                self.calls.append(kwargs)
                if "NextToken" not in kwargs:
                    return {"Parameters": [], "NextToken": "page-2"}
                return {"Parameters": [{"Type": "SecureString", "KeyId": KEY}]}

        aws = object.__new__(v.AwsSources)
        aws._client_error, aws._botocore_error, aws._ssm = Boom, Boom, Ssm()
        self.assertEqual(aws.metadata("/facilitator/rpc-mainnet/base"), ("SecureString", KEY))
        self.assertEqual(aws._ssm.calls[1].get("NextToken"), "page-2")
        aws._ssm.describe_parameters = lambda **kwargs: {"Parameters": []}
        self.assertIsNone(aws.metadata("/facilitator/rpc-mainnet/base"))

    @unittest.skipUnless(importlib.util.find_spec("botocore"), "botocore not installed")
    def test_a_botocore_failure_is_no_verdict_and_names_no_account(self):
        from botocore.exceptions import BotoCoreError, ClientError, NoCredentialsError, ParamValidationError

        aws = object.__new__(v.AwsSources)
        aws._client_error, aws._botocore_error = ClientError, BotoCoreError

        def invalid(**kwargs):
            raise ParamValidationError(report="Invalid name " + SSM + "/facilitator/x")

        def no_credentials(**kwargs):
            raise NoCredentialsError()

        def gone(**kwargs):
            raise ClientError({"Error": {"Code": "ParameterNotFound", "Message": ACCOUNT}}, "GetParameter")

        for call, expected in ((invalid, "ParamValidationError"), (no_credentials, "NoCredentialsError")):
            with self.assertRaises(v.ReadFailed) as failed:
                aws._call(call, ())
            self.assertEqual(str(failed.exception), expected)
        with self.assertRaises(v.NotFound) as missing:
            aws._call(gone, ("ParameterNotFound",))
        self.assertNotIn(ACCOUNT, str(missing.exception))


def b58encode(raw: bytes) -> str:
    number, out = int.from_bytes(raw, "big"), ""
    while number:
        number, rest = divmod(number, 58)
        out = v.BASE58[rest] + out
    return "1" * (len(raw) - len(raw.lstrip(b"\x00"))) + out


class Addresses(Base):
    @unittest.skipUnless(importlib.util.find_spec("eth_account"), "eth_account not installed")
    def test_an_evm_key_written_differently_is_still_a_difference(self):
        f, old_td, new_td, values = world()
        key = secrets.token_hex(32)
        f.secrets[SM + "facilitator-evm-mainnet-private-key-" + SUFFIX] = json.dumps({"private_key": "0x" + key})
        f.params[SSM + "/facilitator/evm-mainnet-private-key/private_key"] = key
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertIn("| DIFIERE |", row(out, "EVM_PRIVATE_KEY_MAINNET"))
        self.assertIn("misma direccion", row(out, "EVM_PRIVATE_KEY_MAINNET"))
        f.params[SSM + "/facilitator/evm-mainnet-private-key/private_key"] = secrets.token_hex(32)
        code, out = run(f, old_td, new_td)
        self.assertIn("OTRA DIRECCION", row(out, "EVM_PRIVATE_KEY_MAINNET"))
        self.assertNoLeak(out, values + [key])

    @unittest.skipUnless(importlib.util.find_spec("cryptography"), "cryptography not installed")
    def test_a_solana_key_written_differently_is_still_a_difference(self):
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

        seed = secrets.token_bytes(32)
        public = Ed25519PrivateKey.from_private_bytes(seed).public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        keypair = b58encode(seed + public)
        f, old_td, new_td, values = world()
        name = SSM + "/facilitator/solana-mainnet-keypair/private_key"
        secret = SM + "facilitator-solana-mainnet-keypair-" + SUFFIX
        f.secrets[secret] = json.dumps({"private_key": keypair})
        f.params[name] = keypair + " "
        f.meta["/facilitator/solana-mainnet-keypair/private_key"] = ("SecureString", KEY)
        old_td[0]["secrets"].append({"name": "SOLANA_PRIVATE_KEY_MAINNET", "valueFrom": secret + ":private_key::"})
        new_td[0]["secrets"].append({"name": "SOLANA_PRIVATE_KEY_MAINNET", "valueFrom": name})
        code, out = run(f, old_td, new_td)
        self.assertEqual(code, 1)
        self.assertIn("| DIFIERE |", row(out, "SOLANA_PRIVATE_KEY_MAINNET"))
        self.assertIn("misma direccion", row(out, "SOLANA_PRIVATE_KEY_MAINNET"))
        self.assertNoLeak(out, values + [keypair])


if __name__ == "__main__":
    unittest.main()
