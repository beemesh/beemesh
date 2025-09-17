BIN_DIR=bin

all: build

build:
	mkdir -p $(BIN_DIR)
	go build -o $(BIN_DIR)/beemesh ./cmd/machine
	go build -o $(BIN_DIR)/workload ./cmd/workload
	go build -o $(BIN_DIR)/beectl ./cmd/beectl

clean:
	rm -rf $(BIN_DIR)
