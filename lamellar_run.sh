#!/bin/bash
rm -rf /dev/shm/lamellar_*  2> /dev/null #cleanup incase any previous run failed unexpectedly

NUMPES=1
NPROC=`nproc --all`

for i in "$@"; do
  case $i in
    -N=*|--numpes=*)
    NUMPES="${i#*=}"
    shift
    ;;
    -T=*|--threads-per-pe=*)
    THREADS="${i#*=}"
    shift
    ;;
  esac
done

bin=$1

THREADS=${THREADS:=$((NPROC/NUMPES))}

ENDPE=$(( $NUMPES-1))
JOBID=$((1+ $RANDOM % 1000 ))
S_CORE=$((0))
E_CORE=$(($S_CORE + $THREADS))
for pe in $(seq 0 $ENDPE); do
  # echo "pe: $pe s_core $S_CORE e_core: $((E_CORE-1)) nthreads=${THREADS} nproc $NPROC ${@:2}"
  if [ "$E_CORE" -gt "$NPROC" ]; then
    echo "more threads ${E_CORE} than cores ${NPROC} "
    exit
  fi
  LAMELLAE_BACKEND="shmem" LAMELLAR_MEM_SIZE=$((1*1024*1024*1024)) LAMELLAR_THREADS=$((THREADS-1)) LAMELLAR_NUM_PES=$NUMPES LAMELLAR_PE_ID=$pe LAMELLAR_JOB_ID=$JOBID taskset --cpu-list $S_CORE-$((E_CORE-1))  $bin  "${@:2}"  &
  S_CORE=$(($E_CORE ))
  E_CORE=$(($S_CORE + $THREADS))
done

wait
