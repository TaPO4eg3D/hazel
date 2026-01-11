# Hazel Protocol Specification

## Notation in diagrams

```
one byte:
+--------+
|        |
+--------+

a variable number of bytes:
+========+
|        |
+========+
```

## Request/Response

This message format is identical for both request and response
since communication is happening through a single TCP connection.

When a client expecting a response from the server, it has to match
it to a stored UUID

```
                                u16      u32
+----------+=====+-----------+======+===========+======+
| KEY_SIZE | KEY | IS_TAGGED | UUID | BODY_SIZE | BODY |
+----------+=====+-----------+======+===========+======+
```

- `KEY` is an RPC method name
- `UUID` is needed to correctly identify to which request
we're getting a response to. This is needed because of highly
asyncronous design
- `UUID` is present only if `IS_TAGGED` is set to true
