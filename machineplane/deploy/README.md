Create the podman network before running the machine.yml
```bash
podman network create --subnet 10.90.0.10/24 beemesh-net
```
Also ensure that podman.sock is available
```
systemctl start podman.socket
```
Then run
```
podman kube play --network beemesh-net --ip 10.90.0.50 --ip 10.90.0.51 --ip 10.90.0.52 machine.yml
```
The machine agent expects an explicit Podman socket argument. Make sure each container spec includes `--podman-socket /run/podman/podman.sock` (or your chosen socket path) so the runtime uses only the mounted socket.

Verify peers
```
curl 10.90.0.50:3000/debug/dht/peers
```

Apply manifest
```
./target/debug/beectl --api-url http://10.90.0.50:3000 apply -f tests/sample_manifests/nginx.yml
```

Verify status for a "beemesh-${id}-pod"
```
podman pod ls
```

Delete manifest
```
./target/debug/beectl --api-url http://10.90.0.50:3000 delete -f tests/sample_manifests/nginx.yml
```