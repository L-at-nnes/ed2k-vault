# Reference fixtures

Drop files here together with the link eMule generated for them. For every
`name.ext`, add `name.ext.expected` containing the single ed2k link, copied from
eMule with the **hashset** and **AICH** options enabled.

`cargo test -p ed2k-vault-core --test fixtures` hashes each file and compares
the result with eMule field by field (name, size, ed2k hash, `h=`, `p=`).
The test passes trivially while no `.expected` file is present.

Useful cases:

- a file under 1 MB, a few MB, and several hundred MB;
- a file whose size is an exact multiple of 9 728 000 bytes (for example
  `9 728 000 * N` bytes of any content);
- a file over 4 GB;
- names with spaces, a double space, accents and special characters.

Large files do not belong in git: list them in `.gitignore` or keep only the
`.expected` files and point `ED2K_VAULT_FIXTURES` at a directory holding the
data.
