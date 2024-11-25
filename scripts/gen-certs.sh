#!/usr/bin/env bash

# this script generates the mTLS certs required for the server and a handful of clients.

set -eu

BASE=$(git rev-parse --show-toplevel)
DIR=$BASE/data/tls

# clean up what was already in the dir
rm -f "$DIR"/*.{key,pem,csr,srl}

mkdir -p data/tls

# create new CA certs for the client and server

gen_ca_pair() {
    local kind="$1"
    openssl genrsa -out "$DIR/${kind}_ca.key" 2048
    openssl req -x509 -new -nodes -key "$DIR/${kind}_ca.key" -sha256 -days 1826 -out "$DIR/${kind}_ca.pem" -subj "/CN=${kind} root CA"
}
gen_ca_pair client
gen_ca_pair server

# generate a signed key pair for the server

gen_server_cert_pair() {
    local name="server"
    local num=""
    local ca_key="$DIR/${name}_ca.key"
    local ca_pem="$DIR/${name}_ca.pem"
    local key="$DIR/${name}${num}.key"
    local csr="$DIR/${name}${num}.csr"
    local pem="$DIR/${name}${num}.pem"
    openssl genrsa -out "$key" 2048
    openssl req -new -key "$key" -out "$csr"  -subj "/CN=${name}${num}" -addext "subjectAltName = DNS:example.com, DNS:*.example.com, DNS:localhost, IP:127.0.0.1, IP:::1"
    openssl x509 -req -in "$csr" -CA "$ca_pem" -CAkey "$ca_key" -CAcreateserial -out "$pem" -days 1826 -sha256 -extfile <(printf "subjectAltName=DNS:example.com,DNS:*.example.com,DNS:localhost,IP:127.0.0.1,IP:::1")
}
gen_server_cert_pair

# generate a number of client signed key pairs

gen_client_cert_pair() {
    local name="client"
    local num="$1"
    local ca_key="$DIR/${name}_ca.key"
    local ca_pem="$DIR/${name}_ca.pem"
    local key="$DIR/${name}${num}.key"
    local csr="$DIR/${name}${num}.csr"
    local pem="$DIR/${name}${num}.pem"
    openssl genrsa -out "$key" 2048
    openssl req -new -key "$key" -out "$csr"  -subj "/CN=${name}${num}"
    openssl x509 -req -in "$csr" -CA "$ca_pem" -CAkey "$ca_key" -CAcreateserial -out "$pem" -days 1826 -sha256
}
gen_client_cert_pair 1
gen_client_cert_pair 2
gen_client_cert_pair 3


# clean up the leftovers

rm -f "$DIR"/*.{srl,csr}
