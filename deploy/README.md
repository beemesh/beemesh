Ensure that podman.sock is available
```
systemctl start podman.socket
```
Then run
```
    podman kube play deploy/machine.yml
```
The machine agent expects an explicit Podman socket argument. Make sure each container spec includes `--podman-socket /run/podman/podman.sock` (or your chosen socket path) so the runtime uses only the mounted socket.

Verify peers
```
curl localhost:3000/debug/dht/peers
```

Apply manifest via Kubernetes compatibility layer
```
kubectl --server http://localhost:3000 apply -f tests/sample_manifests/nginx.yml
```

Verify status for a "beemesh-${id}-pod"
```
podman pod ls
```

Deletion not yet supported
```
# returns a 501-style error because Machineplane cannot tear workloads down yet
kubectl --server http://localhost:3000 delete -f tests/sample_manifests/nginx.yml
```
