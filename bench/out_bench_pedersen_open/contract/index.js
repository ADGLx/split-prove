import * as __compactRuntime from '@midnight-ntwrk/compact-runtime';
__compactRuntime.checkRuntimeVersion('0.16.0');

const _descriptor_0 = new __compactRuntime.CompactTypeBytes(32);

const _descriptor_1 = new __compactRuntime.CompactTypeUnsignedInteger(65535n, 2);

class _tuple_0 {
  alignment() {
    return _descriptor_0.alignment().concat(_descriptor_1.alignment());
  }
  fromValue(value_0) {
    return [
      _descriptor_0.fromValue(value_0),
      _descriptor_1.fromValue(value_0)
    ]
  }
  toValue(value_0) {
    return _descriptor_0.toValue(value_0[0]).concat(_descriptor_1.toValue(value_0[1]));
  }
}

const _descriptor_2 = new _tuple_0();

const _descriptor_3 = __compactRuntime.CompactTypeJubjubPoint;

const _descriptor_4 = __compactRuntime.CompactTypeField;

const _descriptor_5 = new __compactRuntime.CompactTypeUnsignedInteger(18446744073709551615n, 8);

const _descriptor_6 = __compactRuntime.CompactTypeBoolean;

class _Either_0 {
  alignment() {
    return _descriptor_6.alignment().concat(_descriptor_0.alignment().concat(_descriptor_0.alignment()));
  }
  fromValue(value_0) {
    return {
      is_left: _descriptor_6.fromValue(value_0),
      left: _descriptor_0.fromValue(value_0),
      right: _descriptor_0.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_6.toValue(value_0.is_left).concat(_descriptor_0.toValue(value_0.left).concat(_descriptor_0.toValue(value_0.right)));
  }
}

const _descriptor_7 = new _Either_0();

const _descriptor_8 = new __compactRuntime.CompactTypeUnsignedInteger(340282366920938463463374607431768211455n, 16);

class _ContractAddress_0 {
  alignment() {
    return _descriptor_0.alignment();
  }
  fromValue(value_0) {
    return {
      bytes: _descriptor_0.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_0.toValue(value_0.bytes);
  }
}

const _descriptor_9 = new _ContractAddress_0();

const _descriptor_10 = new __compactRuntime.CompactTypeUnsignedInteger(255n, 1);

export class Contract {
  witnesses;
  constructor(...args_0) {
    if (args_0.length !== 1) {
      throw new __compactRuntime.CompactError(`Contract constructor: expected 1 argument, received ${args_0.length}`);
    }
    const witnesses_0 = args_0[0];
    if (typeof(witnesses_0) !== 'object') {
      throw new __compactRuntime.CompactError('first (witnesses) argument to Contract constructor is not an object');
    }
    this.witnesses = witnesses_0;
    this.circuits = {
      bench_pedersen_open: (...args_1) => {
        if (args_1.length !== 4) {
          throw new __compactRuntime.CompactError(`bench_pedersen_open: expected 4 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const sk_lo_0 = args_1[1];
        const sk_hi_0 = args_1[2];
        const r_0 = args_1[3];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.currentQueryContext != undefined)) {
          __compactRuntime.typeError('bench_pedersen_open',
                                     'argument 1 (as invoked from Typescript)',
                                     'bench_pedersen_open.compact line 10 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(typeof(sk_lo_0) === 'bigint' && sk_lo_0 >= 0 && sk_lo_0 <= __compactRuntime.MAX_FIELD)) {
          __compactRuntime.typeError('bench_pedersen_open',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'bench_pedersen_open.compact line 10 char 1',
                                     'Field',
                                     sk_lo_0)
        }
        if (!(typeof(sk_hi_0) === 'bigint' && sk_hi_0 >= 0 && sk_hi_0 <= __compactRuntime.MAX_FIELD)) {
          __compactRuntime.typeError('bench_pedersen_open',
                                     'argument 2 (argument 3 as invoked from Typescript)',
                                     'bench_pedersen_open.compact line 10 char 1',
                                     'Field',
                                     sk_hi_0)
        }
        if (!(typeof(r_0) === 'bigint' && r_0 >= 0 && r_0 <= __compactRuntime.MAX_FIELD)) {
          __compactRuntime.typeError('bench_pedersen_open',
                                     'argument 3 (argument 4 as invoked from Typescript)',
                                     'bench_pedersen_open.compact line 10 char 1',
                                     'Field',
                                     r_0)
        }
        const context = { ...contextOrig_0, gasCost: __compactRuntime.emptyRunningCost() };
        const partialProofData = {
          input: {
            value: _descriptor_4.toValue(sk_lo_0).concat(_descriptor_4.toValue(sk_hi_0).concat(_descriptor_4.toValue(r_0))),
            alignment: _descriptor_4.alignment().concat(_descriptor_4.alignment().concat(_descriptor_4.alignment()))
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = this._bench_pedersen_open_0(context,
                                                     partialProofData,
                                                     sk_lo_0,
                                                     sk_hi_0,
                                                     r_0);
        partialProofData.output = { value: [], alignment: [] };
        return { result: result_0, context: context, proofData: partialProofData, gasCost: context.gasCost };
      }
    };
    this.impureCircuits = {
      bench_pedersen_open: this.circuits.bench_pedersen_open
    };
    this.provableCircuits = {
      bench_pedersen_open: this.circuits.bench_pedersen_open
    };
  }
  initialState(...args_0) {
    if (args_0.length !== 1) {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 1 argument (as invoked from Typescript), received ${args_0.length}`);
    }
    const constructorContext_0 = args_0[0];
    if (typeof(constructorContext_0) !== 'object') {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 'constructorContext' in argument 1 (as invoked from Typescript) to be an object`);
    }
    if (!('initialZswapLocalState' in constructorContext_0)) {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 'initialZswapLocalState' in argument 1 (as invoked from Typescript)`);
    }
    if (typeof(constructorContext_0.initialZswapLocalState) !== 'object') {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 'initialZswapLocalState' in argument 1 (as invoked from Typescript) to be an object`);
    }
    const state_0 = new __compactRuntime.ContractState();
    let stateValue_0 = __compactRuntime.StateValue.newArray();
    stateValue_0 = stateValue_0.arrayPush(__compactRuntime.StateValue.newNull());
    state_0.data = new __compactRuntime.ChargedState(stateValue_0);
    state_0.setOperation('bench_pedersen_open', new __compactRuntime.ContractOperation());
    const context = __compactRuntime.createCircuitContext(__compactRuntime.dummyContractAddress(), constructorContext_0.initialZswapLocalState.coinPublicKey, state_0.data, constructorContext_0.initialPrivateState);
    const partialProofData = {
      input: { value: [], alignment: [] },
      output: undefined,
      publicTranscript: [],
      privateTranscriptOutputs: []
    };
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_10.toValue(0n),
                                                                                              alignment: _descriptor_10.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_3.toValue(({x: 0n, y: 1n})),
                                                                                              alignment: _descriptor_3.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    state_0.data = new __compactRuntime.ChargedState(context.currentQueryContext.state.state);
    return {
      currentContractState: state_0,
      currentPrivateState: context.currentPrivateState,
      currentZswapLocalState: context.currentZswapLocalState
    }
  }
  _ecAdd_0(a_0, b_0) {
    const result_0 = __compactRuntime.ecAdd(a_0, b_0);
    return result_0;
  }
  _ecMul_0(a_0, b_0) {
    const result_0 = __compactRuntime.ecMul(a_0, b_0);
    return result_0;
  }
  _hashToCurve_0(value_0) {
    const result_0 = __compactRuntime.hashToCurve(_descriptor_2, value_0);
    return result_0;
  }
  _bench_pedersen_open_0(context, partialProofData, sk_lo_0, sk_hi_0, r_0) {
    const G_sk_lo_0 = this._hashToCurve_0([new Uint8Array([109, 105, 100, 110, 105, 103, 104, 116, 58, 115, 112, 108, 105, 116, 45, 115, 107, 45, 108, 111, 91, 118, 49, 93, 0, 0, 0, 0, 0, 0, 0, 0]),
                                           0n]);
    const G_sk_hi_0 = this._hashToCurve_0([new Uint8Array([109, 105, 100, 110, 105, 103, 104, 116, 58, 115, 112, 108, 105, 116, 45, 115, 107, 45, 104, 105, 91, 118, 49, 93, 0, 0, 0, 0, 0, 0, 0, 0]),
                                           0n]);
    const G_r_0 = this._hashToCurve_0([new Uint8Array([109, 105, 100, 110, 105, 103, 104, 116, 58, 115, 112, 108, 105, 116, 45, 98, 108, 105, 110, 100, 91, 118, 49, 93, 0, 0, 0, 0, 0, 0, 0, 0]),
                                       0n]);
    const C_0 = this._ecAdd_0(this._ecAdd_0(this._ecMul_0(G_sk_lo_0, sk_lo_0),
                                            this._ecMul_0(G_sk_hi_0, sk_hi_0)),
                              this._ecMul_0(G_r_0, r_0));
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_10.toValue(0n),
                                                                                              alignment: _descriptor_10.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_3.toValue(C_0),
                                                                                              alignment: _descriptor_3.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return [];
  }
}
export function ledger(stateOrChargedState) {
  const state = stateOrChargedState instanceof __compactRuntime.StateValue ? stateOrChargedState : stateOrChargedState.state;
  const chargedState = stateOrChargedState instanceof __compactRuntime.StateValue ? new __compactRuntime.ChargedState(stateOrChargedState) : stateOrChargedState;
  const context = {
    currentQueryContext: new __compactRuntime.QueryContext(chargedState, __compactRuntime.dummyContractAddress()),
    costModel: __compactRuntime.CostModel.initialCostModel()
  };
  const partialProofData = {
    input: { value: [], alignment: [] },
    output: undefined,
    publicTranscript: [],
    privateTranscriptOutputs: []
  };
  return {
  };
}
const _emptyContext = {
  currentQueryContext: new __compactRuntime.QueryContext(new __compactRuntime.ContractState().data, __compactRuntime.dummyContractAddress())
};
const _dummyContract = new Contract({ });
export const pureCircuits = {};
export const contractReferenceLocations =
  { tag: 'publicLedgerArray', indices: { } };
//# sourceMappingURL=index.js.map
