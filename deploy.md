podman build -t beemesh-machineplane -f machineplane/Dockerfile .

Machineplane manifest
```
apiVersion: v1
kind: Pod
metadata:
  name: beemesh-machineplane
  labels:
    app: beemesh-machineplane
spec:
  restartPolicy: Always
  containers:
    - name: bootstrap
      image: beemesh-machineplane:latest
      env:
      - name: RUST_LOG
        value: "debug"
      args:
        - "--libp2p-quic-port"
        - "4001"
        - "--disable-scheduling"
        - "--disable-machine-api"
      ports:
        - containerPort: 4001
          hostPort: 4001
          protocol: UDP
        - containerPort: 4001
          hostPort: 4001
          protocol: TCP
        - containerPort: 3000
          hostPort: 3000
          protocol: TCP
        - containerPort: 3000
          hostPort: 3000
          protocol: UDP
    - name: peer1-with-dns
      image: beemesh-machineplane:latest
      env:
      - name: RUST_LOG
        value: "debug"
      args:
        - "--disable-rest-api"
        - "--disable-machine-api"
        - "--bootstrap-peer"
        - "/ip4/127.0.0.1/udp/4001/quic-v1"
        - "--podman-socket"
        - "/run/podman/podman.sock"
      volumeMounts:
        - name: podman-socket
          mountPath: /run/podman/podman.sock
        - name: host-proc-cpuinfo
          mountPath: /host-proc/cpuinfo
          readOnly: true
        - name: host-proc-meminfo
          mountPath: /host-proc/meminfo
          readOnly: true
        - name: host-containers-storage
          mountPath: /var/lib/containers/storage
          readOnly: true
    - name: peer2-with-localhost
      image: beemesh-machineplane:latest
      env:
      - name: RUST_LOG
        value: "debug"
      args:
        - "--disable-rest-api"
        - "--disable-machine-api"
        - "--bootstrap-peer"
        - "/ip4/127.0.0.1/udp/4001/quic-v1"
        - "--podman-socket"
        - "/run/podman/podman.sock"
      volumeMounts:
        - name: podman-socket
          mountPath: /run/podman/podman.sock
        - name: host-proc-cpuinfo
          mountPath: /host-proc/cpuinfo
          readOnly: true
        - name: host-proc-meminfo
          mountPath: /host-proc/meminfo
          readOnly: true
        - name: host-containers-storage
          mountPath: /var/lib/containers/storage
          readOnly: true
  volumes:
    - name: podman-socket
      hostPath:
        path: /run/user/1000/podman/podman.sock
        type: Socket
    - name: host-proc-cpuinfo
      hostPath:
        path: /proc/cpuinfo
        type: File
    - name: host-proc-meminfo
      hostPath:
        path: /proc/meminfo
        type: File
    - name: host-containers-storage
      hostPath:
        path: /var/lib/containers/storage
        type: Directory
```

Ensure that podman.sock is available
```
systemctl start podman.socket
```
Then run
```
    podman kube play deploy/machineplane.yml
```
The machineplane agent expects an explicit Podman socket argument. Make sure each container spec includes `--podman-socket /run/podman/podman.sock` (or your chosen socket path) so the runtime uses only the mounted socket.

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

Delete manifest
```
kubectl --server http://localhost:3000 delete -f tests/sample_manifests/nginx.yml
```

