# Vectores Hedera y spike del SDK de Rust (fase 0)

**Veredicto: GO.** El crate publicado `hiero-sdk 0.45.0` decodifica un payload
real de `@x402/hedera`, lo cofirma y lo vuelve a serializar **sin tocar un solo
byte** del cuerpo que firmó el pagador ni de sus firmas previas, en las siete
variantes por nodo que emite el cliente oficial, con Ed25519, ECDSA secp256k1,
KeyList y umbral 2 de 3. Medido el 2026-09-16, no leído.

La corrida completa está en `RESULTS-2026-09-16.txt`. El relato, con lo que
implica para las fases 1 a 5, está en
`docs/handoffs/2026-09-16-x4-hedera-spike.md`.

Nada de lo que hay aquí toca fondos, cuentas reales ni mainnet. Las cuentas son
sintéticas (`0.0.1001` pagador, `0.0.2002` destino, `0.0.3003` patrocinador) para
que ningún vector pueda confundirse con el patrocinador real de otro facilitador,
y las claves se **derivan** de etiquetas ASCII: no hay material de clave guardado
en el repositorio.

## Reproducir

```bash
# 1. Vectores (Node 20+; sin red, sin fondos, sin mainnet)
cd tests/hedera-e2e
npm install
node generate-vectors.mjs

# 2. El experimento (requiere protoc; ver abajo)
cd rust-spike
cargo test            # 15 aserciones: el veredicto, como tests
cargo run             # el informe legible
cargo run -- --json   # el informe en JSON
```

`package.json` fija sin acento circunflejo `@x402/hedera 2.26.0`,
`@hiero-ledger/sdk 2.85.0` y `@hiero-ledger/proto 2.31.0`, que son las versiones
publicadas el día de la medición. El paquete oficial se movió de 2.25.0 a 2.26.0
entre la investigación y esta corrida: el `package.json` dice la que se midió, no
la que se planificó. El `package-lock.json` **no** se versiona, siguiendo lo que
ya hacen los otros directorios de pruebas JS del repositorio, así que las
dependencias transitivas pueden moverse; las tres directas no. El `Cargo.lock`
del spike sí se versiona, como el del facilitador, porque el veredicto es una
medición y su árbol de dependencias forma parte de ella.

**`protoc` es obligatorio.** Sin él, `cargo build` falla en el build script de
`hiero-sdk-proto 0.22.0`, que genera el código con `tonic-build` y no trae
compilador vendorizado:

```
error: failed to run custom build command for `hiero-sdk-proto v0.22.0`
  Error: Could not find `protoc`. ...
```

`brew install protobuf` en macOS, `apt-get install -y protobuf-compiler` en
Debian. Sólo hace falta donde se **compila**: la etapa de runtime de la imagen
sólo copia un binario ya enlazado.

## El spike de Rust

`rust-spike/` es un crate aparte, con su propio `[workspace]`, deliberadamente
**fuera** del workspace del facilitador: no toca su `Cargo.lock` ni su árbol de
dependencias. Nada de esto se despliega. Su `cargo build` emite tres avisos de
`patch ... was not used` porque hereda el `[patch.crates-io]` de `.cargo/config.toml`
del repositorio; son inofensivos.

Por vector hace seis pasos, y el sexto es la pregunta entera:

1. decodifica el base64 que enviaría un pagador real;
2. inspecciona **todas** las variantes por nodo en protobuf, no la primera;
3. decodifica con `hiero-sdk` y contrasta lo que el SDK dice;
4. verifica la firma del pagador sobre los cuerpos congelados;
5. cofirma como patrocinador;
6. reserializa, vuelve a decodificar y compara **byte a byte**.

## Qué prueba cada vector

| Vector | Variantes | Qué demuestra |
|---|---|---|
| `01-hbar-ed25519-official-client` | 7 | El camino real: `@x402/hedera` congela contra `Client.forTestnet()` y firma. Un payload legítimo **no** tiene una variante, tiene siete. |
| `02-hbar-ed25519-multinode` | 4 | La línea base reproducible byte a byte. |
| `03-hts-usdc-ecdsa-multinode` | 3 | HTS (USDC testnet `0.0.429274`, 6 decimales) con firmante ECDSA secp256k1. |
| `04-hbar-threshold-2of3` | 2 | Umbral 2 de 3 (Ed25519, Ed25519, ECDSA) firmado por dos: se acepta sin exigir la tercera. |
| `05-hbar-keylist-all` | 3 | KeyList sin umbral: firman todos sus miembros. |
| `06-adversarial-second-variant-repointed` | 3 | La variante 1 paga a otra cuenta **con firma válida** sobre ese cuerpo. |
| `07-adversarial-second-variant-stale-signature` | 3 | La variante 1 multiplica el importe por 1000 y conserva la firma vieja. |
| `08-adversarial-wrong-signer` | 1 | Firmada por una clave que no gobierna la cuenta debitada. |
| `09-adversarial-second-variant-other-transaction-id` | 3 | La variante 1 lleva **otro** transaction id, correctamente firmado. |
| `10-adversarial-empty-pubkey-prefix` | 2 | `pubKeyPrefix` vacío: prefijo de toda clave pública. |
| `11-adversarial-is-approval-debit` | 2 | El débito lleva `isApproval=true`: gasto de allowance, no del pagador. |
| `12-adversarial-nft-rider` | 2 | Transferencia de USDC que arrastra un NFT a un tercero. |
| `13-adversarial-allowance-hook` | 2 | `preTxAllowanceHook` sobre el débito: un contrato corre antes de la transferencia. |
| `14-adversarial-duplicate-node` | 3 | Dos variantes nombran el mismo nodo. |
| `15-adversarial-fee-payer-debited` | 2 | El destino cobra todo, pero el 40% sale del patrocinador. |

## La línea divisoria que hay que llevarse a la fase 2

De los diez adversariales, **el SDK rechaza cuatro y cofirma seis**. Los cuatro
que rechaza (06, 07, 09, 10) son listas cuyas variantes se contradicen o cuyo
mapa de firmas es inconsistente; `AnyTransaction::from_bytes` las mira todas y
falla. Los otros (08, 11, 12, 13, 14, 15) son transferencias de Hedera
perfectamente válidas: el SDK las decodifica, el pagador de verdad las firmó, y
el SDK las cofirma sin decir nada.

Por eso `rust-spike/src/policy.rs` existe: es la inspección que el SDK no hace y
no le corresponde hacer. Y por eso lee protobuf y no los getters del SDK —
`get_hbar_transfers()` devuelve `HashMap<AccountId, Hbar>`, que no tiene dónde
poner `isApproval` ni un hook. `policy.rs` no es código de producción; es la
prueba de que las reglas se pueden expresar sobre los bytes que llegan.
