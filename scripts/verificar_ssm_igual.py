#!/usr/bin/env python3
"""Check, in memory, that every SSM parameter the facilitator's new task definition reads holds
exactly what the old one injects from Secrets Manager. Prints only ok / DIFIERE per variable.

WHY THIS EXISTS
---------------
Owner's decision 266 (terraform/environments/production/secrets.tf): the task definition stops
reading Secrets Manager and reads SSM SecureStrings under /facilitator/, copied from the secrets on
2026-10-16. Secrets Manager picks a field out of a JSON secret (`<arn>:<field>::`); SSM cannot, so
every field became a parameter of its own. Before the new revision runs -- and long before any
original is deleted -- each copy has to hold the bytes ECS injects today. A plan cannot see that;
this does.

WHAT IT COMPARES
----------------
The OLD task definition against the NEW one, variable by variable, by NAME:
  old  Secrets Manager valueFrom -> GetSecretValue, then the JSON field it names (or the whole
       string when it names none);
  new  SSM valueFrom -> GetParameter(WithDecryption=True).
Both values stay inside this process and are compared with hmac.compare_digest over their UTF-8
bytes. For EVM and Solana private keys the address of each side is derived as well, when
eth_account / cryptography are installed: "misma direccion" next to a DIFIERE means the same key
written differently, which is still a DIFIERE -- ECS would inject other bytes.

It also reports, by name only, any difference in the environment, the image or the set of secret
names between the two revisions (the new revision must change nothing but where its secrets come
from), and each parameter's type and KMS key (ssm:DescribeParameters, metadata only).

A COPY OLDER THAN ITS SECRET IS A DIFFERENCE, IN EVERY MODE
-----------------------------------------------------------
Each copy has to have been written strictly after its secret last changed: the parameter's
LastModifiedDate (DescribeParameters) against the secret's LastChangedDate (DescribeSecret).
Otherwise the row is COPIA VIEJA and the exit code 1, even when the bytes happen to match -- writing
the parameter again clears it. This is the risk of the migration itself (a value written only to
Secrets Manager no longer reaches the task), it reads no value, and so --solo-metadatos catches it
too. Measured 2026-10-31: facilitator-rpc-testnet changed on 2026-10-30, two weeks after its three
copies, and the comparison by name alone printed "metadatos en orden".

WHAT IT NEVER PRINTS
--------------------
No value, no part of one, no length, no hash. Variable, secret and parameter names only, with the
account id and the secret's random suffix masked. AWS errors print their code; anything that
could quote a value (a JSON error quotes the document) prints only its type.

USAGE
-----
    python3 scripts/verificar_ssm_igual.py --viejo facilitator-production:482 --nuevo plan.json
    python3 scripts/verificar_ssm_igual.py --viejo td-482.json --nuevo facilitator-production:483
    python3 scripts/verificar_ssm_igual.py --viejo ... --nuevo ... --solo-metadatos

--viejo / --nuevo take a file -- `aws ecs describe-task-definition` output, `terraform show -json`
of a plan, or a bare containerDefinitions list -- or a family:revision read from ECS.
--solo-metadatos reads no value at all: names, sources, environment, image, parameter metadata and
the dates of secrets and copies, which is what an identity without kms:Decrypt can check.

Permissions: ecs:DescribeTaskDefinition (family:revision only), secretsmanager:DescribeSecret,
secretsmanager:GetSecretValue, ssm:GetParameter + kms:Decrypt on the parameters' key,
ssm:DescribeParameters. --solo-metadatos needs neither GetSecretValue nor GetParameter/kms:Decrypt.

Exit code: 0 every pair equal and nothing else changed; 1 something differs or is missing;
2 no verdict: nothing differs, but a read failed (credentials, network, access) or an input
could not be interpreted.
"""

from __future__ import annotations

import argparse
import hmac
import json
import os
import re
import sys
from dataclasses import dataclass
from datetime import datetime
from typing import Optional

REGION = "us-east-2"
CONTAINER = "facilitator"
KEY_ALIAS = "alias/facilitator-bazaar-admin-token"
PLAN_ADDRESS = "aws_ecs_task_definition.facilitator"

ACCOUNT = re.compile(r"(?<![0-9])[0-9]{12}(?![0-9])")
SECRET_SUFFIX = re.compile(r"-[A-Za-z0-9]{6}$")
BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


class NotFound(Exception):
    """The secret, the field or the parameter does not exist."""


class ReadFailed(Exception):
    """A read did not complete (access denied, throttling...). Carries a printable code."""


class BadInput(Exception):
    """A task definition or a valueFrom this script cannot interpret. No verdict."""


def mask(text: str) -> str:
    return ACCOUNT.sub("<cuenta>", text)


# ---------------------------------------------------------------------------------------------
# Sources: AWS in production, a stand-in in tests/scripts/test_verificar_ssm_igual.py
# ---------------------------------------------------------------------------------------------

class AwsSources:
    def __init__(self, region: str):
        import boto3
        from botocore.exceptions import BotoCoreError, ClientError

        self._client_error = ClientError
        self._botocore_error = BotoCoreError
        self._sm = boto3.client("secretsmanager", region_name=region)
        self._ssm = boto3.client("ssm", region_name=region)
        self._ecs = boto3.client("ecs", region_name=region)

    def _call(self, fn, missing: tuple[str, ...], **kwargs):
        try:
            return fn(**kwargs)
        except self._client_error as error:
            code = error.response.get("Error", {}).get("Code", "ClientError")
            if code in missing:
                raise NotFound(code) from None
            raise ReadFailed(code) from None
        except self._botocore_error as error:
            # No credentials, no endpoint, a timeout, a parameter botocore refused: only the type,
            # because a ParamValidationError quotes the ARN it was given, account id included.
            raise ReadFailed(type(error).__name__) from None

    def secret(self, secret_id: str, stage: str, version: str) -> str:
        kwargs = {"SecretId": secret_id}
        if stage:
            kwargs["VersionStage"] = stage
        if version:
            kwargs["VersionId"] = version
        answer = self._call(self._sm.get_secret_value, ("ResourceNotFoundException",), **kwargs)
        if "SecretString" not in answer:
            raise ReadFailed("SecretBinary")
        return answer["SecretString"]

    def changed(self, secret_id: str) -> datetime:
        """When the secret last changed in any way (a value, a staging label). Metadata only."""
        answer = self._call(self._sm.describe_secret, ("ResourceNotFoundException",), SecretId=secret_id)
        when = answer.get("LastChangedDate") or answer.get("CreatedDate")
        if when is None:
            raise ReadFailed("DescribeSecret sin fecha")
        return when

    def parameter(self, name: str) -> str:
        answer = self._call(self._ssm.get_parameter, ("ParameterNotFound",), Name=name, WithDecryption=True)
        return answer["Parameter"]["Value"]

    def metadata(self, name: str) -> Optional[tuple[str, str, Optional[datetime]]]:
        # DescribeParameters may answer an empty page with a NextToken even for an Equals filter:
        # only the last page can say "does not exist".
        kwargs = {"ParameterFilters": [{"Key": "Name", "Option": "Equals", "Values": [name]}]}
        while True:
            answer = self._call(self._ssm.describe_parameters, (), **kwargs)
            found = answer.get("Parameters", [])
            if found:
                return found[0].get("Type", ""), found[0].get("KeyId", ""), found[0].get("LastModifiedDate")
            if not answer.get("NextToken"):
                return None
            kwargs["NextToken"] = answer["NextToken"]

    def task_definition(self, task_definition: str) -> list:
        answer = self._call(self._ecs.describe_task_definition, ("ClientException",),
                            taskDefinition=task_definition)
        return answer["taskDefinition"]["containerDefinitions"]


# ---------------------------------------------------------------------------------------------
# Task definitions and valueFrom
# ---------------------------------------------------------------------------------------------

def load_containers(spec: str, sources) -> list:
    """containerDefinitions from a file (three shapes) or from ECS by family:revision."""
    if not os.path.isfile(spec):
        return sources.task_definition(spec)
    with open(spec, encoding="utf-8") as handle:
        try:
            doc = json.load(handle)
        except ValueError:
            raise BadInput(f"{spec}: not JSON") from None
    if isinstance(doc, list):
        return doc
    if "taskDefinition" in doc:
        return doc["taskDefinition"]["containerDefinitions"]
    if "resource_changes" in doc:
        for change in doc["resource_changes"]:
            if change.get("address") == PLAN_ADDRESS:
                after = (change.get("change") or {}).get("after") or {}
                defs = after.get("container_definitions")
                if defs is None:
                    raise BadInput(f"{spec}: {PLAN_ADDRESS} has no planned container_definitions")
                return json.loads(defs) if isinstance(defs, str) else defs
        raise BadInput(f"{spec}: the plan does not change {PLAN_ADDRESS}")
    raise BadInput(f"{spec}: not a task definition, a plan or a containerDefinitions list")


def container(defs: list, name: str) -> dict:
    for item in defs:
        if item.get("name") == name:
            return item
    raise BadInput(f"no container named {name!r}")


@dataclass(frozen=True)
class Source:
    kind: str           # "sm" or "ssm"
    ref: str            # what the read takes: the secret ARN, or the parameter ARN / name
    name: str           # printable: the secret's logical name, or the parameter's name
    field: str = ""     # Secrets Manager JSON key, "" for the whole string
    stage: str = ""
    version: str = ""

    def label(self) -> str:
        if self.kind == "sm":
            return f"SM {self.name}" + (f":{self.field}" if self.field else "")
        return f"SSM {self.name}"


def parse_value_from(value_from: str) -> Source:
    """ECS valueFrom -> where it reads. Secrets Manager only by ARN, SSM by ARN or by name."""
    parts = value_from.split(":")
    if value_from.startswith("arn:") and len(parts) >= 7 and parts[2] == "secretsmanager" and parts[5] == "secret":
        logical = SECRET_SUFFIX.sub("", parts[6])
        extra = parts[7:] + ["", "", ""]
        return Source("sm", ":".join(parts[:7]), logical, extra[0], extra[1], extra[2])
    if value_from.startswith("arn:") and len(parts) >= 6 and parts[2] == "ssm" and parts[5].startswith("parameter/"):
        rest = value_from.split(":parameter", 1)[1]          # "/facilitator/x"
        name = rest if "/" in rest[1:] else rest[1:]          # a flat name has no leading slash
        return Source("ssm", value_from, name)
    if value_from.startswith("arn:"):
        raise BadInput(f"unsupported valueFrom: {mask(value_from)}")
    return Source("ssm", value_from, value_from)


# ---------------------------------------------------------------------------------------------
# Optional address derivation (bool only)
# ---------------------------------------------------------------------------------------------

def _b58decode(text: str) -> bytes:
    number = 0
    for char in text:
        number = number * 58 + BASE58.index(char)
    body = number.to_bytes((number.bit_length() + 7) // 8, "big")
    return b"\x00" * (len(text) - len(text.lstrip("1"))) + body


def address_of(variable: str, value: str) -> Optional[str]:
    """The public address a private key controls, or None when unknown / not derivable here."""
    try:
        if variable.startswith("EVM_PRIVATE_KEY"):
            from eth_account import Account

            return Account.from_key(value.strip()).address.lower()
        if variable.startswith("SOLANA_PRIVATE_KEY"):
            from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
            from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

            keypair = _b58decode(value.strip())
            if len(keypair) != 64:
                return None
            public = Ed25519PrivateKey.from_private_bytes(keypair[:32]).public_key()
            return public.public_bytes(Encoding.Raw, PublicFormat.Raw).hex()
    except Exception:  # noqa: BLE001 -- a key that does not parse has no address; never echo it
        return None
    return None


# ---------------------------------------------------------------------------------------------
# The comparison
# ---------------------------------------------------------------------------------------------

class Report:
    def __init__(self):
        self.differs = False
        self.undecided = False
        self.lines: list[str] = []

    def row(self, verdict: str, *cells: str) -> None:
        if verdict not in ("ok", "sin cambio", "omitido"):
            if verdict == "ERROR":
                self.undecided = True
            else:
                self.differs = True
        self.lines.append(mask("| " + " | ".join((verdict,) + cells) + " |"))

    def note(self, text: str, differs: bool = False) -> None:
        self.differs = self.differs or differs
        self.lines.append(mask(text))

    def exit_code(self) -> int:
        if self.differs:
            return 1
        return 2 if self.undecided else 0


def read_old(source: Source, sources) -> str:
    raw = sources.secret(source.ref, source.stage, source.version) if source.kind == "sm" \
        else sources.parameter(source.ref)
    if source.kind != "sm" or not source.field:
        return raw
    try:
        document = json.loads(raw)
    except ValueError:
        raise ReadFailed("secret is not JSON") from None
    if not isinstance(document, dict) or source.field not in document:
        raise NotFound("field")
    value = document[source.field]
    if not isinstance(value, str):
        raise ReadFailed("field is not a string")
    return value


def compare(old_defs: list, new_defs: list, sources, *, values: bool, key_alias: str,
            name: str = CONTAINER) -> Report:
    report = Report()
    old, new = container(old_defs, name), container(new_defs, name)

    same_image = old.get("image") == new.get("image")
    report.note(f"imagen: {'igual' if same_image else 'DIFIERE'}", differs=not same_image)

    old_env = {e["name"]: e.get("value") for e in old.get("environment", [])}
    new_env = {e["name"]: e.get("value") for e in new.get("environment", [])}
    changed = sorted(n for n in set(old_env) | set(new_env) if old_env.get(n) != new_env.get(n)
                     or (n in old_env) != (n in new_env))
    report.note(f"environment: {len(new_env)} variables, "
                + (f"DIFIEREN: {', '.join(changed)}" if changed else "todas iguales"),
                differs=bool(changed))

    old_secrets = {s["name"]: s["valueFrom"] for s in old.get("secrets", [])}
    new_secrets = {s["name"]: s["valueFrom"] for s in new.get("secrets", [])}
    report.note(f"secrets: {len(old_secrets)} en el viejo, {len(new_secrets)} en el nuevo")
    report.note("| resultado | variable | origen viejo | origen nuevo | parametro |")
    report.note("|---|---|---|---|---|")
    for gone in sorted(set(old_secrets) - set(new_secrets)):
        report.row("SOLO EN EL VIEJO", gone, parse_value_from(old_secrets[gone]).label(), "-", "-")
    for added in sorted(set(new_secrets) - set(old_secrets)):
        report.row("SOLO EN EL NUEVO", added, "-", parse_value_from(new_secrets[added]).label(), "-")

    changed_at: dict = {}   # secret ARN -> when it last changed: one DescribeSecret per secret
    for variable in sorted(set(old_secrets) & set(new_secrets)):
        before, after = parse_value_from(old_secrets[variable]), parse_value_from(new_secrets[variable])
        meta, written = check_metadata(after, sources, key_alias) if after.kind == "ssm" else ("-", None)
        if after.kind != "ssm":
            report.row("SIGUE EN SECRETS MANAGER", variable, before.label(), after.label(), meta)
            continue
        if meta.startswith("metadatos:"):
            report.row("ERROR", variable, before.label(), after.label(), meta)
            continue
        if meta != "ok":
            report.row(meta.upper(), variable, before.label(), after.label(), meta)
            continue
        if before == after:
            report.row("sin cambio", variable, before.label(), after.label(), meta)
            continue
        if before.kind == "sm":
            try:
                stale = copied_before_last_change(before, written, sources, changed_at)
            except NotFound as missing:
                report.row("FALTA EN EL VIEJO", variable, before.label(), after.label(), str(missing))
                continue
            except ReadFailed as failed:
                report.row("ERROR", variable, before.label(), after.label(), f"fecha: {failed}")
                continue
            if stale:
                report.row("COPIA VIEJA", variable, before.label(), after.label(),
                           "el secreto cambio despues de la copia")
                continue
            meta += "; copia posterior al secreto"
        if not values:
            report.row("omitido", variable, before.label(), after.label(), meta)
            continue
        try:
            old_value = read_old(before, sources)
        except NotFound as missing:
            report.row("FALTA EN EL VIEJO", variable, before.label(), after.label(), str(missing))
            continue
        except ReadFailed as failed:
            report.row("ERROR", variable, before.label(), after.label(), f"viejo: {failed}")
            continue
        try:
            new_value = sources.parameter(after.ref)
        except NotFound:
            report.row("FALTA", variable, before.label(), after.label(), "parametro inexistente")
            continue
        except ReadFailed as failed:
            report.row("ERROR", variable, before.label(), after.label(), f"nuevo: {failed}")
            continue
        equal = hmac.compare_digest(old_value.encode("utf-8"), new_value.encode("utf-8"))
        detail = meta
        old_address, new_address = address_of(variable, old_value), address_of(variable, new_value)
        if old_address is not None and new_address is not None:
            detail += "; misma direccion" if old_address == new_address else "; OTRA DIRECCION"
        del old_value, new_value
        report.row("ok" if equal else "DIFIERE", variable, before.label(), after.label(), detail)
    return report


def check_metadata(source: Source, sources, key_alias: str) -> tuple[str, Optional[datetime]]:
    """(verdict, when the parameter was last written). The date is only meaningful with "ok"."""
    try:
        found = sources.metadata(source.name)
    except ReadFailed as failed:
        return f"metadatos: {failed}", None
    if found is None:
        return "parametro inexistente", None
    kind, key_id, written = found
    if kind != "SecureString":
        return f"no es SecureString ({kind})", None
    if key_id != key_alias and not key_id.endswith(":" + key_alias):
        return "otra llave KMS", None
    return "ok", written


def copied_before_last_change(secret: Source, written: Optional[datetime], sources, changed_at: dict) -> bool:
    """True unless the parameter was written strictly after the secret last changed.

    A tie counts as stale: a real copy is written after the read it copies. A parameter or a secret
    with no date is no verdict (ReadFailed), never "fresh"."""
    if written is None:
        raise ReadFailed("parametro sin fecha")
    if secret.ref not in changed_at:
        changed_at[secret.ref] = sources.changed(secret.ref)
    return changed_at[secret.ref] >= written


def main(argv: Optional[list] = None, sources=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--viejo", required=True, help="the revision that reads Secrets Manager")
    parser.add_argument("--nuevo", required=True, help="the revision that reads SSM")
    parser.add_argument("--solo-metadatos", action="store_true", help="read no value")
    parser.add_argument("--contenedor", default=CONTAINER)
    parser.add_argument("--llave", default=KEY_ALIAS, help="KMS alias every parameter must be on")
    parser.add_argument("--region", default=REGION)
    args = parser.parse_args(argv)

    try:
        if sources is None:
            sources = AwsSources(args.region)
        old_defs, new_defs = load_containers(args.viejo, sources), load_containers(args.nuevo, sources)
        report = compare(old_defs, new_defs, sources, values=not args.solo_metadatos,
                         key_alias=args.llave, name=args.contenedor)
    except (NotFound, ReadFailed, BadInput) as failed:
        print(mask(f"RESULTADO: sin veredicto ({type(failed).__name__}: {failed})"))
        return 2
    except Exception as unexpected:  # noqa: BLE001 -- 1 means "differs"; anything unforeseen is no verdict
        # Only the type: an unforeseen error's text could quote anything it was handed.
        print(f"RESULTADO: sin veredicto (error inesperado: {type(unexpected).__name__})")
        return 2
    for line in report.lines:
        print(line)
    code = report.exit_code()
    ok = "RESULTADO: metadatos en orden, ningun valor comparado" if args.solo_metadatos \
        else "RESULTADO: todo igual"
    print({0: ok, 1: "RESULTADO: hay diferencias o faltantes",
           2: "RESULTADO: sin veredicto, una lectura fallo"}[code])
    return code


if __name__ == "__main__":
    sys.exit(main())
