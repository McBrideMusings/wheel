# ADR-0015: A pairing code carries a one-time token that a phone exchanges for its own credential

Pairing issues a token that works once and expires after five minutes. The phone presents it with the certificate fingerprint it pinned and receives a credential that belongs to that device; the Devices tab lists devices and revokes one without touching the others. The token in the daemon's token file is for local clients, and the remote listener does not accept it.
