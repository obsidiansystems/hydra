#!/bin/sh

. ./foreman/common.sh

wait_for_postgres
wait_for_hydra_db

export HYDRA_DBA="postgres://${USER}@localhost:$HYDRA_PG_PORT/hydra"

exec hydra-drv-daemon --socket -
