### source build ###
FROM golang:1.14-alpine3.12 as build

COPY src /src

WORKDIR /src

RUN set -ex ;\
    apk add git ;\
    go get -d -v -t;\
    CGO_ENABLED=0 GOOS=linux go build -v -o /files/usr/local/bin/beemesh

### runtime build ###
FROM alpine

COPY files /
COPY --from=build /files /

EXPOSE 80 443

