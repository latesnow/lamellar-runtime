#!/bin/bash



root=$PWD
. $root/../prep.rc


## test using local lamellae 
# mkdir -p local_lamellae
# cd local_lamellae
# for toolchain in stable ; do #nightly; do
#  features=""
#  if [ "${toolchain}" = "nightly" ]; then
#    features="--features nightly"
#  fi
# #  cargo clean
#  cargo +$toolchain build --release ${features} --examples
#  mkdir -p ${toolchain}
#  cd ${toolchain}
#  for dir in `ls $root/examples`; do
#    mkdir -p $dir 
#    cd $dir
#      for test in `ls $root/examples/$dir`; do
#        test=`basename $test .rs`
#        LAMELLAR_THREADS=19 srun -N 1 --partition=all --time 0:5:00 $root/target/release/examples/$test > ${test}.out 2>&1  &
#      done
#    cd ..
#  done
#  cd ..
#  wait
# done


### test using rofi shm lamellae
mkdir -p shmem_lamellae
cd shmem_lamellae
for toolchain in stable; do
 features=""
#  if [ "${toolchain}" = "nightly" ]; then
#    features="--features nightly"
#  fi
#  cargo clean
#  cargo +$toolchain build --release --examples
 mkdir -p ${toolchain}
 cd ${toolchain}
 for mode in debug release; do
  mkdir -p $mode
  cd ${mode}
  for dir in `ls $root/examples`; do
    mkdir -p $dir
    cd $dir
      for test in `ls $root/examples/$dir`; do
        test=`basename $test .rs`
        LAMELLAR_MEM_SIZE=$((5 * 1024 * 1024 * 1024)) srun -n 1 -N 1 -A lamellar --time 0:5:00 --mpi=pmi2 $root/lamellar_run.sh -N=1 -T=23 $root/target/${mode}/examples/$test |& tee ${test}_n1.out &
        LAMELLAR_MEM_SIZE=$((5 * 1024 * 1024 * 1024)) srun -n 1 -N 1 -A lamellar --time 0:5:00 --mpi=pmi2 $root/lamellar_run.sh -N=2  -T=11 $root/target/${mode}/examples/$test |& tee ${test}_n2.out &
        LAMELLAR_MEM_SIZE=$((5 * 1024 * 1024 * 1024)) srun -n 1 -N 1 -A lamellar --time 0:5:00 --mpi=pmi2 $root/lamellar_run.sh -N=8 -T=2 $root/target/${mode}/examples/$test |& tee ${test}_n8.out &
      done
    cd ..
  done
  cd ..
  wait
 done
 cd ..
done

### test using rofi verbs lamellae
# mkdir -p rofiverbs_lamellae
# cd rofiverbs_lamellae
# for toolchain in stable; do #nightly; do
#   features=""
#   if [ "${toolchain}" = "nightly" ]; then
#     features="--features nightly"
#   fi
#   # cargo clean
#   cargo +$toolchain build --release --features enable-rofi  --examples
#   mkdir -p ${toolchain}
#   cd ${toolchain}
#   for dir in `ls $root/examples`; do
#     mkdir -p $dir
#     cd $dir
#       for test in `ls $root/examples/$dir`; do
#         test=`basename $test .rs`
#         echo "performing ${test}"
#         LAMELLAE_BACKEND="rofi" LAMELLAR_ROFI_PROVIDER="verbs" LAMELLAR_THREADS=7 srun -N 2 --partition=datavortex --time 0:5:00 --mpi=pmi2 $root/target/release/examples/$test > ${test}_n2.out 2>&1 & 
#       done
#       if [ $dir != "bandwidths" ]; then
#         for test in `ls $root/examples/$dir`; do
#           test=`basename $test .rs`
#           echo "performing ${test}"
#           LAMELLAE_BACKEND="rofi" LAMELLAR_ROFI_PROVIDER="verbs" LAMELLAR_THREADS=7 srun -N 8 --partition=datavortex --time 0:5:00 --mpi=pmi2 $root/target/release/examples/$test > ${test}_n8.out 2>&1 &
#         done
#         for test in `ls $root/examples/$dir`; do
#           test=`basename $test .rs`
#           echo "performing ${test}"
#           LAMELLAE_BACKEND="rofi" LAMELLAR_ROFI_PROVIDER="verbs" LAMELLAR_THREADS=3 srun -n 32 -N 16 --partition=datavortex --time 0:10:00 --mpi=pmi2 $root/target/release/examples/$test > ${test}_n32.out 2>&1 &
#         done
#       fi
#     cd ..
#   done
#   cd ..
#   wait
# done
# #